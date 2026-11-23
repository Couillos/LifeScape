#![forbid(unsafe_code)]
//! One source of prices (2026-11-23): the market's memory
//! (`price_estimate_memory`) answers exactly what the store path
//! (`price_estimate_sql`) does, at the same `now`. Temp database per test;
//! the real-database comparison is `#[ignore]` (`LS_REAL_DB`).

use lifescape_json::{object, str_value, Value};
use lifescape_store::{NewOrder, OrderSide};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;
use crate::rates_price::{price_estimate_memory, price_estimate_sql};

const FEATHER: i64 = 314;
const RUNE: i64 = 556;
const NEVER_BOOK: i64 = 1000;
const NEVER_BARE: i64 = 1001;
const DORMANT_ONLY: i64 = 1002;
const TWIN_LOW: i64 = 1003;
const TWIN_HIGH: i64 = 1004;
const OLD: i64 = 1005;

fn catalogue() -> Vec<CatalogueItem> {
    vec![
        item(FEATHER, "Feather"),
        item(RUNE, "Air rune"),
        item(NEVER_BOOK, "Never traded book"),
        item(NEVER_BARE, "Never traded bare"),
        item(DORMANT_ONLY, "Dormant only"),
        item(TWIN_HIGH, "Twin"),
        item(TWIN_LOW + 5000, "Late"),
        item(TWIN_LOW, "Twin"),
        item(OLD, "Old trade"),
        item(995, "Coins"),
    ]
}

fn item(id: i64, name: &str) -> CatalogueItem {
    CatalogueItem { id, name: name.to_string(), shop_price: 2, alch_value: 1 }
}

fn fund(service: &MarketService, account: &str, item: i64) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(10_000_000)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(100_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{reply:?}");
}

fn place(service: &MarketService, account: &str, side: &str, item: i64, qty: i64, price: i64) {
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{reply:?}");
}

fn trade(service: &MarketService, item: i64, qty: i64, price: i64) {
    place(service, "maker", "sell", item, qty, price);
    place(service, "taker", "buy", item, qty, price);
    crate::market_test_support::settle_matched(service);
}

#[test]
fn memory_and_store_paths_agree() {
    let path = std::env::temp_dir().join(format!("lifescape-rates-price-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let service = MarketService::open(&path, catalogue()).expect("opens");
    for account in ["maker", "taker", "bidder", "asker"] {
        for id in [FEATHER, RUNE, NEVER_BOOK, OLD, TWIN_LOW] {
            fund(&service, account, id);
        }
    }
    // Lot-traded item (lot 100 where lots are on), median of fills, then
    // a dust ask and a deep bid around it.
    for price in [4, 5, 6, 5, 4] {
        trade(&service, RUNE, 100, price);
    }
    place(&service, "asker", "sell", RUNE, 1, 1);
    place(&service, "bidder", "buy", RUNE, 90, 7);
    // Plain item with fills and a resting book on both sides.
    for price in [10, 12, 11] {
        trade(&service, FEATHER, 3, price);
    }
    place(&service, "asker", "sell", FEATHER, 5, 15);
    place(&service, "bidder", "buy", FEATHER, 5, 9);
    // Never traded: with a book (both sides, one side) and without.
    place(&service, "asker", "sell", NEVER_BOOK, 1, 9);
    place(&service, "bidder", "buy", NEVER_BOOK, 500, 3);
    place(&service, "bidder", "buy", TWIN_LOW, 2, 8);
    // Dormant resting orders: stored, never in the price.
    {
        let store = service.lock_store();
        for side in [OrderSide::Buy, OrderSide::Sell] {
            store
                .place_order(&NewOrder { account: "ghost".to_string(), side, item_id: DORMANT_ONLY, quantity: 10, price: 77, dormant: true, clock: 1, ..NewOrder::default() })
                .expect("dormant order");
            store
                .place_order(&NewOrder { account: "ghost".to_string(), side, item_id: FEATHER, quantity: 50, price: 1, dormant: true, clock: 1, ..NewOrder::default() })
                .expect("dormant order");
        }
    }
    // An old trade priced against a recent book.
    trade(&service, OLD, 1, 20);
    // A ring and a newest clock preloaded at open equal the ones the live
    // fills built (same fills read back from the store, same prices).
    let live_rings: Vec<_> = [FEATHER, RUNE, OLD, NEVER_BOOK].iter().map(|id| (service.last_fills(*id), service.newest_fill_clock(*id).is_some())).collect();
    drop(service);
    let reopened = MarketService::open(&path, catalogue()).expect("reopens");
    let reopened_rings: Vec<_> = [FEATHER, RUNE, OLD, NEVER_BOOK].iter().map(|id| (reopened.last_fills(*id), reopened.newest_fill_clock(*id).is_some())).collect();
    assert_eq!(live_rings, reopened_rings, "live vs preloaded");
    let now = MarketService::now_clock();
    for name in ["Feather", "Air rune", "Old trade", "Never traded book", "Twin", "Coins", "Nothing"] {
        assert_eq!(price_estimate_memory(&reopened, name, now), price_estimate_sql(&reopened.lock_store(), name, now), "reopened: {name:?}");
    }
    let service = reopened;
    // Quoted per lot of 100 from here: the per-unit division is read at
    // answer time (the books above were built per unit).
    crate::market_units::lots_on_for_this_test();
    crate::market_units::load_lots(&[(RUNE, 100)]);
    let now = MarketService::now_clock();
    let names = [
        "Feather", "Air rune", "Never traded book", "Never traded bare", "Dormant only", "Twin", "Late", "Old trade", "Coins", "coins", "COINS",
        "Unknown thing", "", "feather", "Air Rune",
    ];
    let mut priced = 0;
    for name in names {
        let memory = price_estimate_memory(&service, name, now);
        let sql = price_estimate_sql(&service.lock_store(), name, now);
        assert_eq!(memory, sql, "item {name:?}: memory {memory:?} vs store {sql:?}");
        priced += 1;
    }
    assert_eq!(priced, names.len());
    // Anchors: the unknown is the prior, coins are a fact, the twin name
    // takes the lowest id, a bare never-traded item is the prior.
    assert_eq!(price_estimate_memory(&service, "Unknown thing", now), (1.0, true));
    assert_eq!(price_estimate_memory(&service, "Coins", now), (1.0, false));
    assert_eq!(price_estimate_memory(&service, "Never traded bare", now), (1.0, true));
    assert_eq!(service.item_id_by_name("Twin"), Some(TWIN_LOW));
    assert!(!price_estimate_memory(&service, "Feather", now).1);
    assert!(price_estimate_memory(&service, "Never traded book", now).1);
    drop(service);
    let _ = std::fs::remove_file(&path);
    for suffix in ["db-journal", "db-wal", "db-shm"] {
        let _ = std::fs::remove_file(path.with_extension(suffix));
    }
}

/// Every item of a copy of the real database (`LS_REAL_DB`): the service
/// loads it as the daemon does, both paths answer for the whole items
/// table at one `now`.
#[test]
#[ignore]
fn memory_and_store_paths_agree_on_the_real_database() {
    let Ok(copy) = std::env::var("LS_REAL_DB") else { return };
    let path = std::path::PathBuf::from(copy);
    let service = MarketService::open(&path, crate::market_seed::catalogue()).expect("opens the copy");
    let items = service.lock_store().list_items().expect("items");
    let started = std::time::Instant::now();
    service.preload_fills().expect("preload");
    eprintln!("preload of the rings and newest clocks: {:?}", started.elapsed());
    let now = MarketService::now_clock();
    let started = std::time::Instant::now();
    let memory: Vec<(f64, bool)> = items.iter().map(|entry| price_estimate_memory(&service, &entry.name, now)).collect();
    let memory_time = started.elapsed();
    let started = std::time::Instant::now();
    let sql: Vec<(f64, bool)> = {
        let store = service.lock_store();
        items.iter().map(|entry| price_estimate_sql(&store, &entry.name, now)).collect()
    };
    let sql_time = started.elapsed();
    // The one known difference: an order in an in-flight deal (`matched`)
    // still rests in the engine's book (its units are not delivered yet),
    // while the store read keeps `status = 'open'` only. Counted apart.
    let in_flight: std::collections::HashSet<i64> = service
        .lock_store()
        .resting_orders()
        .expect("resting orders")
        .into_iter()
        .filter(|order| order.status == lifescape_store::OrderStatus::Matched)
        .map(|order| order.item_id)
        .collect();
    let mut mismatches = 0;
    let mut explained = 0;
    let mut traded = 0;
    for ((entry, memory), sql) in items.iter().zip(&memory).zip(&sql) {
        if !memory.1 {
            traded += 1;
        }
        if memory != sql && service.item_id_by_name(&entry.name).is_some_and(|id| in_flight.contains(&id)) {
            explained += 1;
            eprintln!("in-flight deal on {} (#{}): memory {memory:?} store {sql:?}", entry.name, entry.item_id);
        } else if memory != sql {
            mismatches += 1;
            if mismatches <= 25 {
                eprintln!("MISMATCH {} (#{}): memory {memory:?} store {sql:?}", entry.name, entry.item_id);
            }
        }
    }
    eprintln!("real db: {} items, {traded} priced by a trade, {mismatches} mismatches, {explained} explained by an in-flight deal; memory {memory_time:?}, store {sql_time:?}", items.len());
    assert_eq!(mismatches, 0);
}

/// The memory path never takes the store lock: a thread asking the price
/// (`price_estimate_memory`, `reference_lot`, `reference`) while the test
/// holds `lock_store()` must come back.
#[test]
fn memory_prices_never_wait_for_the_store_lock() {
    let path = std::env::temp_dir().join(format!("lifescape-rates-price-lock-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let service = std::sync::Arc::new(MarketService::open(&path, catalogue()).expect("opens"));
    for account in ["maker", "taker"] {
        fund(&service, account, FEATHER);
    }
    trade(&service, FEATHER, 3, 10);
    let held = service.lock_store();
    let (sender, receiver) = std::sync::mpsc::channel();
    let asker = service.clone();
    std::thread::spawn(move || {
        let now = MarketService::now_clock();
        let memory = price_estimate_memory(&asker, "Feather", now);
        let lot = asker.reference_lot(FEATHER).price;
        let unit = asker.reference(FEATHER).price;
        let unseen = price_estimate_memory(&asker, "Never traded bare", now);
        let _ = sender.send((memory, lot, unit, unseen));
    });
    let answer = receiver.recv_timeout(std::time::Duration::from_secs(10)).expect("a price read waited for the store lock");
    assert_eq!(answer.0, (10.0, false));
    assert_eq!((answer.1, answer.2), (10, 10));
    assert_eq!(answer.3, (1.0, true));
    drop(held);
    drop(service);
    let _ = std::fs::remove_file(&path);
}
