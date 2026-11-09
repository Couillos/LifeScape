#![forbid(unsafe_code)]
//! Fake-client tests: every op through `handle()`, no daemon, no game.
//! Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

fn catalogue() -> Vec<CatalogueItem> {
    vec![
        CatalogueItem { id: 314, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ]
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-market-svc-{}-{tag}-{stamp}.db", std::process::id()))
}

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = temp_db(tag);
    let service = MarketService::open(&path, catalogue()).expect("opens");
    (service, path)
}

fn request(pairs: Vec<(&str, Value)>) -> Value {
    object(pairs)
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&request(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn cleanup(path: &std::path::PathBuf) {
    let _ = std::fs::remove_file(path);
    let journal = path.with_extension("db-journal");
    let _ = std::fs::remove_file(journal);
}

#[test]
fn price_time_priority_and_partial_fills() {
    let (service, path) = service("priority");
    funded(&service, "seller-a");
    funded(&service, "seller-b");
    funded(&service, "buyer");
    let sell = |account: &str, price: i64| {
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(price)),
        ]))
    };
    assert_eq!(sell("seller-a", 5).get("status").as_str(), Some("open"));
    assert_eq!(sell("seller-b", 5).get("status").as_str(), Some("open"));
    // Buyer takes 15: oldest ask first, remainder rests on seller-b.
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(15)),
        ("price", Value::Int(6)),
    ]));
    assert_eq!(bought.get("ok").as_bool(), Some(true));
    assert_eq!(bought.get("fills").as_arr().map(|fills| fills.len()), Some(2));
    assert_eq!(bought.get("deals").as_arr().map(|deals| deals.len()), Some(2));
    // Seller-a filled, seller-b partially: one open order left.
    let orders = service.handle(&request(vec![("op", str_value("my_orders")), ("account", str_value("seller-b"))]));
    assert_eq!(orders.get("orders").as_arr().map(|rows| rows.len()), Some(1));
    assert_eq!(orders.get("orders").as_arr().unwrap()[0].get("left").as_i64(), Some(5));
    cleanup(&path);
}

#[test]
fn amend_is_cancel_and_replace_behind_the_queue() {
    let (service, path) = service("amend");
    funded(&service, "a");
    funded(&service, "b");
    funded(&service, "buyer");
    let place = |account: &str| {
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(5)),
            ("price", Value::Int(5)),
        ]))
    };
    let first = place("a").get("order").as_i64().unwrap();
    place("b");
    let amended = service.handle(&request(vec![
        ("op", str_value("amend")),
        ("account", str_value("a")),
        ("order", Value::Int(first)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(amended.get("ok").as_bool(), Some(true));
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(5)),
        ("price", Value::Int(5)),
    ]));
    let fills = bought.get("fills").as_arr().cloned().unwrap_or_default();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].get("seller").as_str(), Some("b"), "repriced order lost its queue");
    cleanup(&path);
}

#[test]
fn guards_reject_bands_and_naked_orders() {
    let (service, path) = service("guards");
    funded(&service, "poor");
    // No reference yet: shop price anchors at 2; 5x = 10.
    let over = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("poor")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(11)),
    ]));
    assert_eq!(over.get("ok").as_bool(), Some(false));
    // Naked sell: no stock funded on this account.
    let naked = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("ghost")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(5)),
        ("price", Value::Int(2)),
    ]));
    assert_eq!(naked.get("ok").as_bool(), Some(false));
    // Naked buy: no coins.
    let broke = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("ghost")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(100)),
        ("price", Value::Int(2)),
    ]));
    assert_eq!(broke.get("ok").as_bool(), Some(false));
    cleanup(&path);
}

#[test]
fn cancel_closes_and_deal_walks_every_step() {
    let (service, path) = service("deal");
    funded(&service, "maker");
    funded(&service, "taker");
    let placed = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    let order = placed.get("order").as_i64().unwrap();
    let cancelled = service.handle(&request(vec![
        ("op", str_value("cancel")),
        ("account", str_value("maker")),
        ("order", Value::Int(order)),
    ]));
    assert_eq!(cancelled.get("ok").as_bool(), Some(true));
    assert_eq!(
        service
            .handle(&request(vec![
                ("op", str_value("cancel")),
                ("account", str_value("maker")),
                ("order", Value::Int(order)),
            ]))
            .get("ok")
            .as_bool(),
        Some(false),
        "double cancel is loud"
    );
    // Fresh match, then the whole delivery machine.
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("taker")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    let deal = bought.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().unwrap();
    let arrived = |account: &str| {
        service.handle(&request(vec![
            ("op", str_value("deal_arrived")),
            ("account", str_value(account)),
            ("deal", Value::Int(deal)),
        ]))
    };
    assert_eq!(arrived("maker").get("status").as_str(), Some("convened"));
    assert_eq!(arrived("taker").get("status").as_str(), Some("both_arrived"));
    let report = |outcome: &str| {
        service.handle(&request(vec![
            ("op", str_value("deal_report")),
            ("account", str_value("maker")),
            ("deal", Value::Int(deal)),
            ("outcome", str_value(outcome)),
        ]))
    };
    assert_eq!(report("trading").get("status").as_str(), Some("trading"));
    assert_eq!(report("settled").get("status").as_str(), Some("settled"));
    assert_eq!(report("settled").get("ok").as_bool(), Some(false), "settled is terminal");
    cleanup(&path);
}

#[test]
fn every_open_deal_state_fails_with_its_reason() {
    for reason in ["no_show", "mismatch", "death", "timeout", "nav_down"] {
        let (service, path) = service(reason);
        funded(&service, "maker");
        funded(&service, "taker");
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value("maker")),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(4)),
        ]));
        let bought = service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value("taker")),
            ("side", str_value("buy")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(5)),
        ]));
        let deal = bought.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().unwrap();
        let failed = service.handle(&request(vec![
            ("op", str_value("deal_report")),
            ("account", str_value("taker")),
            ("deal", Value::Int(deal)),
            ("outcome", str_value("failed")),
            ("reason", str_value(reason)),
        ]));
        assert_eq!(failed.get("status").as_str(), Some("failed"), "reason {reason}");
        cleanup(&path);
    }
}

#[test]
fn quote_history_and_tick_serve_the_front() {
    let (service, path) = service("front");
    funded(&service, "maker");
    funded(&service, "taker");
    let quote = service.handle(&request(vec![("op", str_value("quote")), ("item", Value::Int(314))]));
    assert_eq!(quote.get("ok").as_bool(), Some(true));
    assert_eq!(quote.get("reference").as_i64(), Some(2), "shop anchors the void");
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("taker")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    let history = service.handle(&request(vec![
        ("op", str_value("history")),
        ("item", Value::Int(314)),
        ("res", str_value("1h")),
    ]));
    assert_eq!(history.get("ok").as_bool(), Some(true));
    assert_eq!(history.get("trades").as_arr().map(|rows| rows.len()), Some(1));
    let tick = service.handle(&request(vec![("op", str_value("tick")), ("since", Value::Int(0))]));
    assert_eq!(tick.get("trades").as_arr().map(|rows| rows.len()), Some(1));
    let book = service.handle(&request(vec![("op", str_value("book")), ("item", Value::Int(314))]));
    assert_eq!(book.get("ok").as_bool(), Some(true));
    cleanup(&path);
}

