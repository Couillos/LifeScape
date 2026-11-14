#![forbid(unsafe_code)]
//! Impact clamp of the current price through the service (user
//! 2026-11-14): one unit listed at 1 coin never prices an item traded in
//! lots of 100 at 1 (the historic 1-coin ratchet), for the live engine
//! (`MarketService::reference`, its guard anchor) and for the store read
//! of the rates (`rates_price::price_estimate`). Temp database per test.

use lifescape_json::{object, str_value, Value};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("lifescape-market-impact-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![
        CatalogueItem { id: 314, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn cleanup(service: MarketService, path: &std::path::PathBuf) {
    drop(service);
    let _ = std::fs::remove_file(path);
    for suffix in ["db-journal", "db-wal", "db-shm"] {
        let _ = std::fs::remove_file(path.with_extension(suffix));
    }
}

fn fund(service: &MarketService, account: &str) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn place(service: &MarketService, account: &str, side: &str, qty: i64, price: i64) -> Value {
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(314)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{account} {side} {qty}@{price}: {reply:?}");
    reply
}

fn reference(service: &MarketService) -> i64 {
    service.handle(&object(vec![("op", str_value("quote")), ("item", Value::Int(314))])).get("reference").as_i64().unwrap_or(-1)
}

fn estimate(service: &MarketService) -> f64 {
    crate::rates_price::price_estimate(&service.lock_store(), "Feather").0
}

/// Three settled lots of 100 at 4, then one unit listed at 1 coin: the
/// price stays 4 (live engine, guard anchor and store read alike); a
/// real ask of 150 units at 2 moves it to 2.
#[test]
fn a_dust_ask_never_moves_a_lot_traded_item() {
    let (service, path) = service("dust-ask");
    for account in ["maker", "taker", "dust", "deep"] {
        fund(&service, account);
    }
    for _ in 0..3 {
        place(&service, "maker", "sell", 100, 4);
        place(&service, "taker", "buy", 100, 4);
        crate::market_test_support::settle_matched(&service);
    }
    assert_eq!(reference(&service), 4);
    place(&service, "dust", "sell", 1, 1);
    assert_eq!(reference(&service), 4, "one unit at 1 coin is not the 100th unit");
    assert_eq!(service.guard_anchor(314), Some(4), "the 5x band keeps reading the impact price");
    assert_eq!(estimate(&service), 4.0, "the rates read the same price from the store");
    place(&service, "deep", "sell", 150, 2);
    assert_eq!(reference(&service), 2, "a real ask below the median is the price now");
    assert_eq!(estimate(&service), 2.0);
    cleanup(service, &path);
}

/// A bid side thinner than the usual lot never lifts the price; a deep
/// one does, to the price of its 100th unit.
#[test]
fn a_thin_bid_never_lifts_a_lot_traded_item() {
    let (service, path) = service("thin-bid");
    for account in ["maker", "taker", "thin", "deep"] {
        fund(&service, account);
    }
    for _ in 0..3 {
        place(&service, "maker", "sell", 100, 4);
        place(&service, "taker", "buy", 100, 4);
        crate::market_test_support::settle_matched(&service);
    }
    place(&service, "thin", "buy", 10, 9);
    assert_eq!(reference(&service), 4, "10 units bid do not lift a lot of 100");
    assert_eq!(estimate(&service), 4.0);
    place(&service, "deep", "buy", 90, 7);
    assert_eq!(reference(&service), 7, "the 100th unit is bid at 7");
    assert_eq!(estimate(&service), 7.0);
    cleanup(service, &path);
}

/// Never traded: the impact size is one unit, the book's best prices
/// apply (midpoint, else the one side).
#[test]
fn never_traded_reads_the_best_prices() {
    let (service, path) = service("never-traded");
    for account in ["seller", "buyer"] {
        fund(&service, account);
    }
    place(&service, "seller", "sell", 1, 9);
    assert_eq!(reference(&service), 9);
    place(&service, "buyer", "buy", 500, 3);
    assert_eq!(reference(&service), 6, "midpoint of 3 and 9");
    assert_eq!(estimate(&service), 6.0);
    assert_eq!(service.guard_anchor(314), None, "a quote never caps an order");
    cleanup(service, &path);
}
