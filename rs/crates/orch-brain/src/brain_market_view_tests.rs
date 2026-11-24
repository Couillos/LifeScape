//! The shared market view (lot 3.2) equals the old per-bot computation at
//! equal book: the books' views for every asker, the 24 h volume at any
//! clock, the reader's own views, and the lot choice of the fixture lives
//! (and, with `LIFESCAPE_LIVES_DIR`, of every real life) with a populated
//! test market behind the thread's `shared()`.

use std::collections::BTreeMap;
use std::path::Path;

use lifescape_json::{object, Value};
use lifescape_orch_market::market_service::MarketService;

use super::{rows_of, shared_books, volume_24h_of, SharedBooks};
use crate::brain_bids::{choose_in, views_from, with_bot_state, MarketReader};
use crate::brain_raw_world_tests::{place, xp_of_level, TestMarket};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound.max(1)
    }
}

const ACCOUNTS: [&str; 6] = ["mv_a", "mv_b", "mv_c", "mv_d", "mv_e", "Mv_A"];

/// Items with a book worth drawing: a production leg exists, tradeable.
fn market_items(count: usize, rng: &mut Rng) -> Vec<(String, i64)> {
    let mut pool: Vec<(String, i64)> = lifescape_gamedata::f2p::f2p_tradeable_objs()
        .iter()
        .filter_map(|obj| obj.name.map(|name| (name.to_string(), obj.obj_id as i64)))
        .filter(|(name, _)| !name.starts_with("cert_") && !lifescape_planner::obtain_legs::production_legs(name).is_empty())
        .collect();
    pool.sort();
    pool.dedup();
    let mut out = Vec::new();
    while out.len() < count && !pool.is_empty() {
        out.push(pool.swap_remove(rng.below(pool.len() as u64) as usize));
    }
    out
}

fn fund(service: &MarketService, items: &[(String, i64)]) {
    let stock: Vec<(i64, i64)> = items.iter().map(|(_, id)| (*id, 1_000)).collect();
    for account in ACCOUNTS {
        service.sync_holdings(account, 50_000_000, &stock);
    }
}

fn items_of(books: &SharedBooks) -> BTreeMap<String, super::ItemBook> {
    books.items.iter().map(|(name, book)| (name.clone(), book.clone())).collect()
}

#[test]
fn the_shared_books_equal_the_per_bot_views_after_every_change() {
    let market = TestMarket::open("mv_books");
    let service = market.service.clone();
    let deps = &crate::brain_deps::BrainDeps::with_market(Some(service.clone()));
    {
        let mut rng = Rng(0xB00C5);
        let items = market_items(30, &mut rng);
        fund(&service, &items);
        let mut previous: Option<u64> = None;
        let mut changes = 0;
        for step in 0..400 {
            let (name, id) = &items[rng.below(items.len() as u64) as usize];
            let account = ACCOUNTS[rng.below(ACCOUNTS.len() as u64) as usize];
            if rng.below(4) == 0 {
                let side = if rng.below(2) == 0 { lifescape_store::OrderSide::Buy } else { lifescape_store::OrderSide::Sell };
                service.cancel_open_because(account, *id, side, "t");
            } else {
                // Prices spread over a few values so ties and crossings happen.
                let side = if rng.below(2) == 0 { "buy" } else { "sell" };
                place(&service, account, side, *id, 1 + rng.below(20) as i64, [5, 6, 7, 40, 41, 900][rng.below(6) as usize]);
            }
            let (generation, ids, rows) = rows_of(&service);
            let reference = SharedBooks::from_rows(generation, ids, &rows);
            let shared = shared_books(&service);
            assert_eq!(items_of(&shared), items_of(&reference), "step {step}: the published copy is stale");
            assert_eq!(shared.ids(), reference.ids(), "step {step}: ids");
            if let Some(before) = previous {
                changes += usize::from(before != shared.generation());
            }
            previous = Some(shared.generation());
            // The old per-bot computation, asker by asker, item by item.
            for asker in ACCOUNTS {
                let old = views_from(&rows, asker);
                for (item, _) in &items {
                    let new = shared.item(item).map(|book| book.view_for(asker));
                    assert_eq!(new, old.get(item).cloned(), "step {step}: view of {item} for {asker}");
                }
                for item in old.keys() {
                    assert!(shared.item(item).is_some(), "step {step}: {item} missing from the shared copy");
                }
            }
            let _ = name;
        }
        assert!(changes > 50, "the generation barely moved ({changes})");
        // A reader sees the same views through the shared copy.
        for asker in ACCOUNTS {
            let (live, old) = (MarketReader::live(asker, deps), MarketReader::live_per_bot_reference(asker, false, deps));
            for (item, _) in &items {
                assert_eq!(live.view(item), old.view(item), "reader view of {item} for {asker}");
            }
            assert_eq!(live.bid_books(), old.bid_books(), "bid books of {asker}");
            let none = |_: &str| None;
            assert_eq!(live.value_books(&none), old.value_books(&none), "value books of {asker}");
        }
    }
}

#[test]
fn the_day_volume_equals_the_store_sum_at_any_clock() {
    let market = TestMarket::open("mv_volume");
    let service = market.service.clone();
    {
        let mut rng = Rng(0x701);
        let items = market_items(8, &mut rng);
        fund(&service, &items);
        let clock = MarketService::now_clock();
        let mut checked = 0;
        for round in 0..6 {
            // Crossing orders, then every deal settled: executions.
            for (_, id) in &items {
                place(&service, "mv_a", "sell", *id, 3 + round, 10);
                place(&service, "mv_b", "buy", *id, 2 + round, 10);
            }
            lifescape_orch_market::market_test_support::settle_matched(&service);
            for (_, id) in &items {
                for shift in [0i64, 3_600, 43_200, 86_390, 86_410, 2 * 86_400] {
                    let at = clock + shift;
                    let want = service.lock_store().volume_between(*id, at - 86_400, at + 1).expect("sum");
                    assert_eq!(volume_24h_of(&service, *id, at), want, "round {round} item {id} at +{shift}s");
                    checked += 1;
                    // A clock before the copy's window falls back to a fresh copy.
                    let early = clock - 40_000;
                    let want = service.lock_store().volume_between(*id, early - 86_400, early + 1).expect("sum");
                    assert_eq!(volume_24h_of(&service, *id, early), want, "round {round} item {id} early");
                }
            }
        }
        assert!(checked > 100);
        let traded: i64 = items.iter().map(|(_, id)| service.lock_store().volume_between(*id, 0, i64::MAX).expect("sum")).sum();
        assert!(traded > 0, "no execution was made: the volume proof is empty");
    }
}

fn live_of(level: u8) -> Value {
    let skills: Vec<Value> = (0u8..23).map(|stat| object(vec![("id", Value::Int(i64::from(stat))), ("xp", Value::Int(xp_of_level(level))), ("level", Value::Int(i64::from(level)))])).collect();
    object(vec![("skills", Value::Arr(skills)), ("inv", Value::Arr(Vec::new())), ("bank", Value::Arr(Vec::new())), ("worn", Value::Arr(Vec::new()))])
}

/// The lot choice (own trade and whole market) of each life, from the
/// shared view and from the per-bot reference, over a populated market.
fn same_choice_for(lives: &[lifescape_planner::Life], seed: u64) -> (usize, usize) {
    let market = TestMarket::open("mv_choice");
    let service = market.service.clone();
    let deps = &crate::brain_deps::BrainDeps::with_market(Some(service.clone()));
    let (mut compared, mut chosen) = (0, 0);
    {
        let mut rng = Rng(seed);
        let items = market_items(120, &mut rng);
        fund(&service, &items);
        // Bids of several accounts at high prices (a lot pays), asks at low.
        for (_, id) in &items {
            for account in ACCOUNTS {
                if rng.below(3) == 0 {
                    place(&service, account, "buy", *id, 1 + rng.below(30) as i64, 50 + rng.below(5_000) as i64);
                }
                if rng.below(4) == 0 {
                    place(&service, account, "sell", *id, 1 + rng.below(30) as i64, 5_000 + rng.below(9_000) as i64);
                }
            }
        }
        for life in lives {
            let live = live_of(*[15u8, 40, 70].get(compared % 3).unwrap_or(&40));
            let rate = crate::brain_bids::bot_rate(life);
            for whole in [false, true] {
                let (new, old) = (MarketReader::live_answering(&life.account, deps), MarketReader::live_per_bot_reference(&life.account, true, deps));
                let from_new = with_bot_state(life, &live, |state| choose_in(life, &new, rate, state, whole), deps);
                let from_old = with_bot_state(life, &live, |state| choose_in(life, &old, rate, state, whole), deps);
                assert_eq!(from_new, from_old, "{} whole={whole}: the choice differs", life.account);
                compared += 1;
                chosen += usize::from(from_new.is_some());
            }
        }
    }
    (compared, chosen)
}

#[test]
fn the_lot_choice_is_the_same_on_the_fixture_lives() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lives");
    let lives = crate::brain_launch_equiv_tests::load_dir(&dir);
    let (compared, chosen) = same_choice_for(&lives, 0xC401CE);
    eprintln!("lot choice: {} lives, {compared} choices compared identical, {chosen} with a lot", lives.len());
    assert!(compared >= 18);
}

/// Every life of `LIFESCAPE_LIVES_DIR` (a copy of `run/lives`, never written).
#[test]
#[ignore = "reads the real lives: LIFESCAPE_LIVES_DIR=/path/to/copy cargo test --profile release-test the_lot_choice_is_the_same_on_the_real_lives -- --ignored --nocapture"]
fn the_lot_choice_is_the_same_on_the_real_lives() {
    let source = std::env::var("LIFESCAPE_LIVES_DIR").expect("set LIFESCAPE_LIVES_DIR to a copy of run/lives");
    let temp = std::env::temp_dir().join(format!("lifescape_view_lives_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("temp dir");
    for entry in std::fs::read_dir(&source).expect("source dir").flatten() {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            std::fs::copy(entry.path(), temp.join(entry.file_name())).expect("copy");
        }
    }
    let lives = crate::brain_launch_equiv_tests::load_dir(&temp);
    let (mut compared, mut chosen) = (0, 0);
    for round in 0..3u64 {
        let (done, found) = same_choice_for(&lives, 0xC401CE + round);
        compared += done;
        chosen += found;
    }
    eprintln!("lot choice (real lives): {} lives, {compared} choices compared identical, {chosen} with a lot", lives.len());
    let _ = std::fs::remove_dir_all(&temp);
}
