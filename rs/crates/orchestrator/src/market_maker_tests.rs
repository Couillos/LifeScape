//! Market makers daemon side (plan marché Q6): universe, slices, capital
//! target, slice assignment, own positions, reconcile.

use super::*;
use lifescape_market::Leg;

#[test]
fn the_universe_splits_into_eleven_stable_slices() {
    assert_eq!(universe().len(), 575, "every F2P object with a book (notes and coins out)");
    let all = slices();
    assert_eq!(all.len(), 11, "575 / 54 per maker");
    let mut seen: Vec<i64> = all.iter().flatten().copied().collect();
    seen.sort_unstable();
    let mut ids: Vec<i64> = universe().iter().map(|item| item.id).collect();
    ids.sort_unstable();
    assert_eq!(seen, ids, "full coverage, no overlap");
    assert!(all.iter().all(|slice| slice.len() <= lifescape_market::slice_capacity()));
    let again = lifescape_market::partition(&ids, 11);
    assert_eq!(again.as_slice(), all, "deterministic");
    let feather = lifescape_gamedata::objs_by_name("feather")[0].obj_id as i64;
    assert_eq!(all.iter().filter(|slice| slice.contains(&feather)).count(), 1);
}

#[test]
fn every_slice_has_a_capital_target() {
    let targets: Vec<i64> = (0..11).map(capital_target).collect();
    eprintln!("capital targets per slice: {targets:?}");
    for target in &targets {
        assert!((20_000..=400_000).contains(target), "{targets:?}");
        assert_eq!(target % 1000, 0);
    }
}

#[test]
fn a_new_maker_takes_the_first_free_slice() {
    assert_eq!(first_free_slice(&[], 11), Some(0));
    assert_eq!(first_free_slice(&[0, 1, 3], 11), Some(2));
    assert_eq!(first_free_slice(&(0..11).collect::<Vec<u32>>(), 11), None);
    let role = new_role(&[0, 1]).expect("slice 2 free");
    assert_eq!((role.slice, role.slices, role.capital_target), (2, 11, capital_target(2)));
    assert_eq!(role.phase, lifescape_planner::MakerPhase::Earning, "born playing");
    assert!(new_role(&(0..11).collect::<Vec<u32>>()).is_err(), "every slice taken: typed refusal");
}

fn deal(buyer: &str, seller: &str, quantity: i64, price: i64) -> lifescape_store::Deal {
    lifescape_store::Deal {
        deal_id: 0,
        buyer: buyer.to_string(),
        seller: seller.to_string(),
        item_id: 314,
        quantity,
        price,
        meeting_x: 0,
        meeting_y: 0,
        meeting_level: 0,
        status: lifescape_store::DealStatus::Settled,
        fail_reason: String::new(),
        deadline_tick: 0,
        created_clock: 0,
        updated_clock: 0,
        taker_side: None,
    }
}

#[test]
fn positions_carry_the_average_cost() {
    let deals = vec![deal("maker", "looter", 10, 4), deal("maker", "looter", 10, 6), deal("fisher", "maker", 5, 9)];
    let book = positions("maker", &deals);
    let feather = book[&314];
    assert_eq!((feather.units, feather.cost), (15, 75), "20 units for 100, 5 sold at the average 5");
    assert_eq!((feather.bought_units, feather.bought_coins, feather.sold_units, feather.sold_coins), (20, 100, 5, 45));
    assert_eq!(feather.cash(), -55);
    let oversold = positions("maker", &[deal("fisher", "maker", 3, 9)]);
    assert_eq!((oversold[&314].units, oversold[&314].cost), (0, 0), "playing-phase stock carries no maker cost");
}

/// The bank never reaches the janitor line on the maker's own stock:
/// 49 slice types held at most (55 - coins - the kit of 4), a held
/// object always requoted.
#[test]
fn a_maker_never_opens_a_type_past_its_bank() {
    assert_eq!(MAX_HELD_TYPES, 49);
    assert!(may_bid(48, 0));
    assert!(!may_bid(49, 0), "a 50th type would fill the bank");
    assert!(may_bid(49, 3), "an object already held is still bought");
}

#[test]
fn reconcile_keeps_amends_or_replaces() {
    let leg = Leg { price: 96, qty: 10 };
    assert_eq!(reconcile(Some(leg), &[]), vec![Move::Place(leg)]);
    assert_eq!(reconcile(Some(leg), &[(7, 96, 10)]), Vec::<Move>::new(), "same price, enough left: kept (its rank too)");
    assert_eq!(reconcile(Some(leg), &[(7, 90, 6)]), vec![Move::Amend(7, 96)]);
    assert_eq!(reconcile(Some(leg), &[(7, 96, 2)]), vec![Move::Cancel(7), Move::Place(leg)], "a sliver goes, the lot comes back");
    assert_eq!(reconcile(Some(leg), &[(7, 96, 10), (8, 95, 3)]), vec![Move::Cancel(8)], "one row per side");
    assert_eq!(reconcile(None, &[(7, 96, 10), (8, 95, 3)]), vec![Move::Cancel(7), Move::Cancel(8)]);
}

/// Wave 3 (plan marché Q6): the merchant role is gone from every source
/// file and every dashboard page; only the load-time migration of old
/// life files names its method (`planner::market_maker`). The tokens are
/// assembled here so this test never matches itself.
#[test]
fn no_merchant_flip_reference_remains() {
    let tokens: Vec<String> = vec![
        ["market:", "flip"].concat(),
        ["flip", "_quote"].concat(),
        ["Archetype::", "Merchant"].concat(),
        ["MERCHANT", "_SHARE"].concat(),
        ["merchant", "_archetype"].concat(),
        ["run", "_flip"].concat(),
        ["pick_flip", "_target"].concat(),
        ["Flip", "Quote"].concat(),
        ["market_", "flip"].concat(),
        ["rich:", "merchant"].concat(),
    ];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    let mut stack = vec![root.join("rs/crates"), root.join("orchestration/static")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable source dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs" || ext == "js" || ext == "html") {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 100, "scanned the sources: {}", files.len());
    let migration = std::path::Path::new("planner/src/market_maker.rs");
    let mut found = Vec::new();
    for path in files {
        if path.ends_with(migration) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for token in &tokens {
            if text.contains(token.as_str()) {
                found.push(format!("{}: {token}", path.display()));
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}
