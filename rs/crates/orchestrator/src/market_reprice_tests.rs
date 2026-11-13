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
    let first = keep_bid(&service, "fighter", FEATHER, 1, 10).expect("placed");
    let StandingBid::Placed { order, price } = first else { panic!("{first:?}") };
    assert_eq!(price, 1, "untraded: opens at the 1-coin reference");
    assert_eq!(bounds_of(&service, order).map(|bounds| bounds.ceiling), Some(10));
    let mut wall = 4_000_000_000.0;
    for _ in 0..3 {
        wall += 200.0;
        reprice_round(&service, wall, 600.0);
    }
    assert_eq!(price_of(&service, "fighter"), Some(4), "three idle windows: 1 -> 4");
    let kept = keep_bid(&service, "fighter", FEATHER, 1, 30).expect("kept");
    assert!(matches!(kept, StandingBid::Kept { price: 4, .. }), "no reset to the reference: {kept:?}");
    let resting = service.lock_store().account_orders("fighter").unwrap_or_default().into_iter().find(|row| row.remaining > 0).expect("rests");
    assert_eq!(bounds_of(&service, resting.order_id).map(|bounds| bounds.ceiling), Some(30), "the ceiling rose with the wallet");
    let shrunk = keep_bid(&service, "fighter", FEATHER, 1, 2).expect("kept");
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
    let bounds = QuoteBounds {
        account: "old_seller".to_string(),
        sell: true,
        floor: 1,
        ceiling: super::SELL_CAP,
        patience_ticks: 1,
        last_change_wall: 0.0,
        last_remaining: 50,
    };
    assert!(adopt(&service, order, bounds.clone()));
    assert!(!adopt(&service, order, bounds), "adopted once");
    reprice_round(&service, 4_000_000_000.0, 600.0);
    assert_eq!(price_of(&service, "old_seller"), Some(8), "idle: walks down");
    cleanup(&path);
}
