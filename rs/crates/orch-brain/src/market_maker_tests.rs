//! Market makers daemon side (plan marché Q6): universe, slices, capital
//! target, slice assignment, own positions, reconcile.

use super::*;
use lifescape_market::Leg;
use lifescape_store::Order;

#[test]
fn the_universe_splits_into_eleven_slices_within_the_bank() {
    assert_eq!(universe().len(), 575, "every F2P object with a book (notes and coins out)");
    let all = slices();
    assert_eq!(all.len(), 11, "575 / 54 per maker");
    let mut seen: Vec<i64> = all.iter().flatten().copied().collect();
    seen.sort_unstable();
    let mut ids: Vec<i64> = universe().iter().map(|item| item.id).collect();
    ids.sort_unstable();
    assert_eq!(seen, ids, "full coverage, no overlap");
    assert!(all.iter().all(|slice| slice.len() <= lifescape_market::slice_capacity()), "{:?}", all.iter().map(Vec::len).collect::<Vec<_>>());
    let again = lifescape_market::partition(&MAKER_CAPS, &priced_universe(), 11);
    assert_eq!(again.as_slice(), all, "deterministic");
    let feather = lifescape_gamedata::objs_by_name("feather")[0].obj_id as i64;
    assert_eq!(all.iter().filter(|slice| slice.contains(&feather)).count(), 1);
}

/// The user's complaint (2026-11-16): slices cut blind to value asked
/// 60 000 to 290 000 coins. Now every slice asks the same capital within
/// 10 % of the one common target, which covers the largest.
#[test]
fn every_slice_asks_the_same_capital_under_one_common_target() {
    let target = common_capital_target();
    let needs: Vec<i64> = (0..11).map(slice_need).collect();
    let counts: Vec<usize> = slices().iter().map(Vec::len).collect();
    eprintln!("common target {target}; slice needs {needs:?}; objects {counts:?}");
    let (low, high) = (*needs.iter().min().expect("slices"), *needs.iter().max().expect("slices"));
    assert!(high <= target, "the common target covers every slice: {needs:?} vs {target}");
    assert!((high - low) * 10 <= target, "balanced within 10 % of the target: {needs:?} vs {target}");
    assert_eq!(target, 103_000, "derived: balanced need 99 190 rounded up to 100 000, then the smallest thousand covering the largest slice");
    let roles: Vec<i64> = (0..11).map(|slice| new_role(&(0..slice).collect::<Vec<u32>>()).expect("free slice").capital_target).collect();
    assert!(roles.iter().all(|role_target| *role_target == target), "one target for every maker: {roles:?}");
}

/// Stability on the real universe: an object added anywhere (anchors
/// drawn from the universe) moves few others: median under 2 %, never
/// more than 10 % of the universe.
#[test]
fn an_object_added_to_the_universe_moves_few_others() {
    let universe = priced_universe();
    let assigned = |slices: &[Vec<i64>]| -> HashMap<i64, usize> { slices.iter().enumerate().flat_map(|(index, slice)| slice.iter().map(move |id| (*id, index))).collect() };
    let before = assigned(slices());
    let mut moves = Vec::new();
    for draw in 0..40_i64 {
        let id = if draw % 2 == 0 { 20_000 + draw } else { (draw * 97) % 4000 };
        if universe.iter().any(|(item, _)| *item == id) {
            continue;
        }
        let anchor = universe[(draw as usize * 37) % universe.len()].1;
        let mut grown = universe.clone();
        grown.push((id, anchor));
        let after = assigned(&lifescape_market::partition(&MAKER_CAPS, &grown, 11));
        moves.push(before.iter().filter(|(item, slice)| after[*item] != **slice).count());
    }
    moves.sort_unstable();
    eprintln!("objects moved per added object: {moves:?}");
    assert!(moves.len() >= 30);
    assert!(moves[moves.len() / 2] * 50 <= universe.len(), "median under 2 %: {moves:?}");
    assert!(moves.iter().all(|moved| moved * 10 <= universe.len()), "never over 10 %: {moves:?}");
}

#[test]
fn a_new_maker_takes_the_first_free_slice() {
    assert_eq!(first_free_slice(&[], 11), Some(0));
    assert_eq!(first_free_slice(&[0, 1, 3], 11), Some(2));
    assert_eq!(first_free_slice(&(0..11).collect::<Vec<u32>>(), 11), None);
    let role = new_role(&[0, 1]).expect("slice 2 free");
    assert_eq!((role.slice, role.slices, role.capital_target), (2, 11, common_capital_target()));
    assert_eq!(role.phase, lifescape_planner::MakerPhase::Earning, "born playing");
    assert!(new_role(&(0..11).collect::<Vec<u32>>()).is_err(), "every slice taken: typed refusal");
}

/// A maker life written before the balanced partition (old slice 8,
/// its own target 131 000), fabricated: never run/.
fn old_maker_file(phase: &str) -> String {
    format!(
        r#"{{"schema":1,"account":"hill_eric","created_at":1,"seed":9,
        "profile":{{"ironman":false,"market_affinity":0.2,"price_patience":0.4,"archetype":"market_maker",
          "maker":{{"slice":8,"slices":11,"capital_target":131000,"phase":"{phase}","capital_at_switch":140000,"switched_at":1700000000}}}},
        "goal":{{"kind":"get_rich"}},"nodes":[],"engagement":null,"active":null,"history":[]}}"#
    )
}

#[test]
fn an_old_maker_life_migrates_to_its_balanced_slice_and_the_common_target() {
    for phase in ["earning", "quoting"] {
        let mut life = lifescape_planner::life_from_value(&lifescape_json::parse(&old_maker_file(phase)).expect("json")).expect("loads");
        assert!(migrate_role(&mut life), "{phase}: the old target changes");
        let role = lifescape_planner::maker_role(&life).expect("still a maker");
        assert_eq!((role.slice, role.slices, role.capital_target), (8, 11, common_capital_target()), "{phase}: slice index kept, common target");
        assert_eq!(slice_items(role.slice), slices()[8], "{phase}: its objects follow the balanced partition");
        let quoting = phase == "quoting";
        assert_eq!(role.phase == lifescape_planner::MakerPhase::Quoting, quoting, "{phase}: the phase never moves");
        assert_eq!((role.capital_at_switch, role.switched_at), (140_000, 1_700_000_000), "{phase}: P&L baseline kept");
        assert!(!migrate_role(&mut life), "{phase}: idempotent");
        let written = lifescape_planner::life_from_value(&lifescape_planner::life_to_value(&life)).expect("reloads");
        assert_eq!(lifescape_planner::maker_role(&written), Some(role), "{phase}: the file keeps the migrated role");
    }
    let producer_file = old_maker_file("earning").replace("\"archetype\":\"market_maker\"", "\"archetype\":\"producer\"");
    let mut producer = lifescape_planner::life_from_value(&lifescape_json::parse(&producer_file).expect("json")).expect("loads");
    assert!(!migrate_role(&mut producer), "a producer has no role to migrate");
}

/// The brain's load path migrates (a fabricated file in a temp dir),
/// and the report and slice scans read the migrated role too.
#[test]
fn the_life_load_and_the_maker_scans_read_the_migrated_role() {
    let dir = std::env::temp_dir().join(format!("lifescape_maker_migration_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("hill_eric.json");
    std::fs::write(&path, old_maker_file("earning")).expect("write");
    let life = crate::brain::read_life_file(&path).expect("loads");
    assert_eq!(lifescape_planner::maker_role(&life).map(|role| role.capital_target), Some(common_capital_target()));
    let rows = crate::market_maker_report::maker_lives(&dir);
    assert_eq!(rows.iter().map(|(account, role)| (account.as_str(), role.slice, role.capital_target)).collect::<Vec<_>>(), vec![("hill_eric", 8, common_capital_target())]);
    assert_eq!(slices_in(&dir), vec![8]);
    std::fs::remove_dir_all(&dir).ok();
}

/// After a repartition a quoting maker's resting quotes on objects that
/// left its slice are cancelled (their stock then sells as a leftover);
/// quotes inside the slice and other motives stay.
#[test]
fn quotes_outside_the_slice_are_stale() {
    let row = |order_id: i64, item_id: i64, motive: &str| lifescape_store::Order {
        order_id,
        item_id,
        motive: motive.to_string(),
        remaining: 5,
        quantity: 5,
        status: lifescape_store::OrderStatus::Open,
        ..lifescape_store::Order::default()
    };
    let rows = vec![row(1, 314, crate::market_lineage::MARKET_MAKE), row(2, 315, crate::market_lineage::MARKET_MAKE), row(3, 316, "surplus"), Order { status: lifescape_store::OrderStatus::Cancelled, ..row(4, 317, crate::market_lineage::MARKET_MAKE) }];
    assert_eq!(stale_quotes(&rows, &[314]), vec![2]);
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

/// User 2026-11-19 (point 1): hana_91 (144 022 coins, 148 893 locked)
/// retried 129 bids a pass the market refused for want of coins. A buy
/// leg goes out only when the free coins cover it: placed whole, a
/// raise by its extra coins; a cut, a sell or a cancel always does.
#[test]
fn a_maker_bid_goes_out_only_when_covered() {
    use super::{move_covered, Move};
    use lifescape_market::Leg;
    let feather = 314;
    assert!(move_covered(feather, true, &Move::Place(Leg { price: 5, qty: 10 }), 50, None));
    assert!(!move_covered(feather, true, &Move::Place(Leg { price: 5, qty: 10 }), 49, None));
    assert!(move_covered(feather, true, &Move::Amend(7, 6), 10, Some((10, 5))), "+1 on 10 units with 10 free");
    assert!(!move_covered(feather, true, &Move::Amend(7, 7), 10, Some((10, 5))), "+20 with 10 free");
    assert!(move_covered(feather, true, &Move::Amend(7, 4), 0, Some((10, 5))), "a cut");
    assert!(move_covered(feather, false, &Move::Place(Leg { price: 5, qty: 10 }), 0, None), "a sell is the stock's matter");
    assert!(move_covered(feather, true, &Move::Cancel(7), 0, None));
}

/// User 2026-11-19: a maker sells only what it holds free. Live rae_stone
/// listed 370 buckets of water with 131 free, refused every pass: the
/// leg now lists the 131; nothing free, nothing listed. Never more.
#[test]
fn a_maker_ask_lists_only_its_free_stock() {
    use super::{sell_leg_within, Move};
    use lifescape_market::Leg;
    let bucket = 1929;
    assert_eq!(sell_leg_within(bucket, Move::Place(Leg { price: 3, qty: 370 }), 131), Some(Move::Place(Leg { price: 3, qty: 131 })));
    assert_eq!(sell_leg_within(bucket, Move::Place(Leg { price: 3, qty: 50 }), 131), Some(Move::Place(Leg { price: 3, qty: 50 })), "covered: unchanged");
    assert_eq!(sell_leg_within(bucket, Move::Place(Leg { price: 3, qty: 7 }), 0), None, "nothing free: no listing");
    assert_eq!(sell_leg_within(bucket, Move::Amend(9, 2), 0), Some(Move::Amend(9, 2)), "a reprice moves no stock");
    assert_eq!(sell_leg_within(bucket, Move::Cancel(9), 0), Some(Move::Cancel(9)));
}

#[test]
fn a_maker_bid_bought_since_the_last_pass_was_served() {
    use lifescape_store::{Deal, DealStatus};
    let deal = |buyer: &str, item_id: i64, created_clock: i64| Deal {
        deal_id: 1,
        buyer: buyer.to_string(),
        seller: "seller".to_string(),
        item_id,
        quantity: 1,
        price: 5,
        meeting_x: 0,
        meeting_y: 0,
        meeting_level: 0,
        status: DealStatus::Settled,
        fail_reason: String::new(),
        deadline_tick: 0,
        created_clock,
        updated_clock: created_clock,
        taker_side: None,
    };
    let deals = vec![deal("maker", 314, 1_000), deal("other", 526, 2_000)];
    assert!(bought_since("maker", 314, &deals, 900.0));
    assert!(!bought_since("maker", 314, &deals, 1_100.0), "an older buy: unserved this pass");
    assert!(!bought_since("maker", 526, &deals, 900.0), "another's buy");
}
