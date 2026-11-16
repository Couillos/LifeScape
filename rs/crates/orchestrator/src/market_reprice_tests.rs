//! Demand-driven repricing on the real service (temp database, no daemon).

use lifescape_json::{object, str_value, Value};

use super::{adopt, bounds_of, keep_bid, reprice_round, tracked, QuoteBounds, StandingBid};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("lifescape-reprice-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![
        CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(FEATHER)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn place(service: &MarketService, pairs: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", str_value("place")), ("item", Value::Int(FEATHER))];
    all.extend(pairs);
    service.handle(&object(all))
}

fn price_of(service: &MarketService, account: &str) -> Option<i64> {
    service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .find(|row| matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0)
        .map(|row| row.price)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

/// An ask that sold during its window walks up; the next idle window
/// walks it back down; never under its reserve.
#[test]
fn filled_asks_climb_idle_asks_fall() {
    let (service, path) = service("asks");
    funded(&service, "maker");
    funded(&service, "taker");
    let ask = place(&service, vec![("account", str_value("maker")), ("side", str_value("sell")), ("qty", Value::Int(10)), ("price", Value::Int(5)), ("floor", Value::Int(4)), ("patience", Value::Int(1))]);
    assert_eq!(ask.get("ok").as_bool(), Some(true), "{ask:?}");
    let take = place(&service, vec![("account", str_value("taker")), ("side", str_value("buy")), ("qty", Value::Int(2)), ("price", Value::Int(5))]);
    assert_eq!(take.get("status").as_str(), Some("filled"), "{take:?}");
    crate::market_test_support::settle_matched(&service);
    let mut wall = 4_000_000_000.0;
    wall += 10.0;
    assert_eq!(reprice_round(&service, wall, 600.0), 1);
    assert_eq!(price_of(&service, "maker"), Some(6), "sold at 5: the rest asks 6");
    wall += 10.0;
    reprice_round(&service, wall, 600.0);
    assert_eq!(price_of(&service, "maker"), Some(5), "idle window: back down");
    wall += 10.0;
    reprice_round(&service, wall, 600.0);
    assert_eq!(price_of(&service, "maker"), Some(4));
    wall += 10.0;
    reprice_round(&service, wall, 600.0);
    assert_eq!(price_of(&service, "maker"), Some(4), "held at the reserve");
    cleanup(&path);
}

/// A standing bid keeps its walk across passes; only its ceiling follows
/// the wallet, and a shrunk ceiling pulls the price down to it.
#[test]
fn standing_bids_keep_their_walk() {
    let (service, path) = service("standing");
    funded(&service, "fighter");
    let first = keep_bid(&service, "fighter", FEATHER, 1, 10, "gear").expect("placed");
    let StandingBid::Placed { order, price } = first else { panic!("{first:?}") };
    assert_eq!(price, 1, "untraded: opens at the 1-coin reference");
    assert_eq!(bounds_of(&service, order).map(|bounds| bounds.ceiling), Some(10));
    let mut wall = 4_000_000_000.0;
    for _ in 0..3 {
        // One default window: 2000 ticks at 600 ms.
        wall += 1200.0;
        reprice_round(&service, wall, 600.0);
    }
    assert_eq!(price_of(&service, "fighter"), Some(4), "three idle windows: 1 -> 4");
    let kept = keep_bid(&service, "fighter", FEATHER, 1, 30, "gear").expect("kept");
    assert!(matches!(kept, StandingBid::Kept { price: 4, .. }), "no reset to the reference: {kept:?}");
    let resting = service.lock_store().account_orders("fighter").unwrap_or_default().into_iter().find(|row| row.remaining > 0).expect("rests");
    assert_eq!(bounds_of(&service, resting.order_id).map(|bounds| bounds.ceiling), Some(30), "the ceiling rose with the wallet");
    let shrunk = keep_bid(&service, "fighter", FEATHER, 1, 2, "gear").expect("kept");
    assert!(matches!(shrunk, StandingBid::Kept { price: 2, .. }), "{shrunk:?}");
    assert_eq!(price_of(&service, "fighter"), Some(2));
    cleanup(&path);
}

/// A listing without bounds (placed before a restart) is adopted once
/// and walks from then on.
#[test]
fn adopted_listings_walk() {
    let (service, path) = service("adopt");
    funded(&service, "old_seller");
    let listed = place(&service, vec![("account", str_value("old_seller")), ("side", str_value("sell")), ("qty", Value::Int(50)), ("price", Value::Int(9))]);
    let order = listed.get("order").as_i64().expect("order");
    assert_eq!(tracked(&service), 0, "fixed price: no bounds");
    let row = service.lock_store().get_order(order).ok().flatten().expect("listed row");
    let bounds = QuoteBounds { patience_ticks: 1, last_change_wall: 0.0, listed_wall: 0.0, ..QuoteBounds::for_row(&row, 1, super::SELL_CAP) };
    assert!(adopt(&service, order, bounds.clone()));
    assert!(!adopt(&service, order, bounds.clone()), "adopted once");
    // Idempotent even when its bounds are dropped (2026-11-14: hundreds
    // of re-adoptions per bot per pass in the live log).
    service.lock_engine().quote_bounds.remove(&order);
    assert!(super::handled(&service, order));
    assert!(!adopt(&service, order, bounds.clone()), "never re-adopted");
    service.lock_engine().quote_bounds.insert(order, bounds);
    reprice_round(&service, 4_000_000_000.0, 600.0);
    assert_eq!(price_of(&service, "old_seller"), Some(8), "idle: walks down");
    cleanup(&path);
}

/// Daemon CPU (2026-11-14): a resting order inside its patience window
/// (default 2000 ticks) is not touched; one parked at its reserve never
/// amends (its id stays, no new row).
#[test]
fn reprice_work_follows_change_not_order_count() {
    assert_eq!(super::PATIENCE_TICKS, 2000);
    assert_eq!(lifescape_market::zip::FAST_FILL_TICKS, super::PATIENCE_TICKS, "a fast fill is one inside the first window");
    let (service, path) = service("cadence");
    funded(&service, "parked");
    let listed = place(&service, vec![("account", str_value("parked")), ("side", str_value("sell")), ("qty", Value::Int(10)), ("price", Value::Int(5)), ("floor", Value::Int(5))]);
    let order = listed.get("order").as_i64().expect("order");
    let start = bounds_of(&service, order).map(|bounds| bounds.last_change_wall).expect("walks");
    assert_eq!(reprice_round(&service, start + 1000.0 * 0.6, 600.0), 0, "1000 ticks: inside the window");
    assert_eq!(reprice_round(&service, start + 2100.0 * 0.6, 600.0), 0, "at its reserve: parked");
    let rows = service.lock_store().account_orders("parked").unwrap_or_default();
    assert_eq!(rows.len(), 1, "no amend, no replacement row: {rows:?}");
    assert_eq!(rows[0].order_id, order);
    cleanup(&path);
}

/// Plan marché M1/M2 (C1, C3): a need bid walked by amends keeps its
/// lineage (root, t0, motive, no true cancel), and the next obtain on the
/// object takes that bid over instead of cancelling it and re-placing it
/// lower.
#[test]
fn a_need_bid_keeps_its_lineage_across_amends_and_obtains() {
    let (service, path) = service("lineage");
    funded(&service, "fiona_park");
    let first = super::keep_bid_from(&service, "fiona_park", FEATHER, 5, 1, 10, crate::market_lineage::NEED).expect("placed");
    let StandingBid::Placed { order: root, .. } = first else { panic!("{first:?}") };
    let root_row = service.lock_store().get_order(root).ok().flatten().expect("root row");
    let mut wall = 4_000_000_000.0;
    for _ in 0..2 {
        wall += 1200.0;
        reprice_round(&service, wall, 600.0);
    }
    assert_eq!(price_of(&service, "fiona_park"), Some(3), "two idle windows: 1 -> 3");
    let kept = super::keep_bid_from(&service, "fiona_park", FEATHER, 5, 1, 10, crate::market_lineage::NEED).expect("kept");
    let StandingBid::Kept { order, price: 3 } = kept else { panic!("taken over at its walked price: {kept:?}") };
    let row = service.lock_store().get_order(order).ok().flatten().expect("walked row");
    assert_eq!((row.root_id, row.root_clock, row.motive.as_str()), (root, root_row.created_clock, "need"));
    assert!(row.replaces.is_some(), "an amend links its predecessor");
    assert_eq!(bounds_of(&service, order).map(|bounds| (bounds.root_id, bounds.root_clock, bounds.motive)), Some((root, root_row.created_clock, "need".to_string())));
    let rows = service.lock_store().account_orders("fiona_park").unwrap_or_default();
    assert_eq!(rows.len(), 1, "one resting bid, never a second lineage: {rows:?}");
    let replaced = service.lock_store().get_order(root).ok().flatten().expect("root row");
    assert_eq!(replaced.cancel_reason, lifescape_store::AMENDED_REASON, "the walk is no cancellation");
    cleanup(&path);
}

/// Plan marché M1: after a daemon restart the engine's bounds are gone;
/// the kept bid's new bounds read its lineage and t0 from the carnet,
/// never restart them at the adoption.
#[test]
fn adoption_after_a_restart_reads_t0_from_the_carnet() {
    let (service, path) = service("restart");
    funded(&service, "ryan_mason");
    let first = super::keep_bid_from(&service, "ryan_mason", FEATHER, 1, 1, 30, crate::market_lineage::GEAR).expect("placed");
    let StandingBid::Placed { order: root, .. } = first else { panic!("{first:?}") };
    reprice_round(&service, 4_000_001_200.0, 600.0);
    let walked = service.lock_store().account_orders("ryan_mason").unwrap_or_default().into_iter().next().expect("walked");
    assert_ne!(walked.order_id, root, "amended once");
    drop(service);
    let catalogue = vec![
        CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    let reopened = MarketService::open(&path, catalogue).expect("reopens");
    assert_eq!(tracked(&reopened), 0, "bounds died with the daemon");
    funded(&reopened, "ryan_mason");
    let kept = super::keep_bid_from(&reopened, "ryan_mason", FEATHER, 1, 1, 30, crate::market_lineage::GEAR).expect("kept");
    let StandingBid::Kept { order, .. } = kept else { panic!("{kept:?}") };
    let bounds = bounds_of(&reopened, order).expect("adopted");
    assert_eq!((bounds.root_id, bounds.root_clock, bounds.motive.as_str()), (root, walked.root_clock, "gear"));
    assert_eq!(bounds.horizon_ticks, crate::market_lineage::horizon_ticks("gear", false));
    cleanup(&path);
}
