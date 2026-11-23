#![forbid(unsafe_code)]
//! Buy/sell volume split (user 2026-11-13): "quand un mec achete il
//! consomme des ordres de vente, et inversement pour les vendeurs". The
//! order that consumes the book is the taker; its side rides the deal,
//! the settled execution, the candles (`vb`/`vs`) and the live prints
//! (`s`). Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_store::OrderSide;

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("lifescape-market-taker-{}-{tag}-{stamp}.db", std::process::id()));
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

fn place(service: &MarketService, account: &str, side: &str, qty: i64, price: i64) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(FEATHER)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]))
}

fn sum_of(candles: &[Value], key: &str) -> i64 {
    candles.iter().map(|candle| candle.get(key).as_i64().expect("candle key")).sum()
}

#[test]
fn taker_side_reaches_deal_execution_candles_and_prints() {
    let (service, path) = service("split");
    for account in ["maker-ask", "buyer", "maker-bid", "dumper"] {
        funded(&service, account);
    }
    // A buyer consumes a resting ask: buy volume.
    assert_eq!(place(&service, "maker-ask", "sell", 10, 4).get("status").as_str(), Some("open"));
    assert_eq!(place(&service, "buyer", "buy", 10, 5).get("status").as_str(), Some("filled"));
    // A seller hits a resting bid: sell volume.
    assert_eq!(place(&service, "maker-bid", "buy", 4, 3).get("status").as_str(), Some("open"));
    assert_eq!(place(&service, "dumper", "sell", 4, 3).get("status").as_str(), Some("filled"));
    let sides: Vec<Option<OrderSide>> =
        service.lock_store().recent_deals(10).expect("deals").iter().rev().map(|deal| deal.taker_side).collect();
    assert_eq!(sides, vec![Some(OrderSide::Buy), Some(OrderSide::Sell)], "deals carry the taker side");
    // A match is a promise: nothing prints before the handover settles.
    let history = || service.handle(&object(vec![("op", str_value("history")), ("item", Value::Int(FEATHER)), ("res", str_value("1h"))]));
    assert!(history().get("candles").as_arr().is_none_or(|candles| candles.is_empty()));
    assert_eq!(crate::market_test_support::settle_matched(&service), 2);
    let executions = service.lock_store().executions_for_item(FEATHER, 0, i64::MAX).expect("executions");
    let mut settled: Vec<(i64, Option<OrderSide>)> = executions.iter().map(|exec| (exec.quantity, exec.taker_side)).collect();
    settled.sort_by_key(|(quantity, _)| *quantity);
    assert_eq!(settled, vec![(4, Some(OrderSide::Sell)), (10, Some(OrderSide::Buy))], "settled executions keep the side");
    let reply = history();
    let candles = reply.get("candles").as_arr().cloned().unwrap_or_default();
    assert_eq!((sum_of(&candles, "v"), sum_of(&candles, "vb"), sum_of(&candles, "vs")), (14, 10, 4), "{candles:?}");
    let mut trade_sides: Vec<(i64, String)> = reply
        .get("trades")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|trade| (trade.get("q").as_i64().unwrap_or(0), trade.get("s").as_str().unwrap_or("").to_string()))
        .collect();
    trade_sides.sort();
    assert_eq!(trade_sides, vec![(4, "sell".to_string()), (10, "buy".to_string())]);
    let tick = service.handle(&object(vec![("op", str_value("tick")), ("since", Value::Int(0))]));
    let mut live: Vec<(i64, String)> = tick
        .get("trades")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|trade| (trade.get("q").as_i64().unwrap_or(0), trade.get("s").as_str().unwrap_or("").to_string()))
        .collect();
    live.sort();
    assert_eq!(live, vec![(4, "sell".to_string()), (10, "buy".to_string())], "WS live tail carries the side");
    let _ = std::fs::remove_file(&path);
}

/// An execution settled before the split (taker unknown) stays in the
/// total `v`, never in `vb`/`vs`, and its print says `s: null`.
#[test]
fn unknown_side_counts_in_total_only() {
    let (service, path) = service("unknown");
    let clock = MarketService::now_clock();
    service
        .lock_store()
        .record_execution(&lifescape_store::NewExecution {
            item_id: FEATHER,
            quantity: 6,
            price: 3,
            clock,
            server_tick: clock,
            taker_side: None,
        })
        .expect("prints");
    let reply = service.handle(&object(vec![("op", str_value("history")), ("item", Value::Int(FEATHER)), ("res", str_value("1h"))]));
    let candles = reply.get("candles").as_arr().cloned().unwrap_or_default();
    assert_eq!((sum_of(&candles, "v"), sum_of(&candles, "vb"), sum_of(&candles, "vs")), (6, 0, 0), "{candles:?}");
    let trades = reply.get("trades").as_arr().cloned().unwrap_or_default();
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].get("s"), &Value::Null, "unknown side is null, never guessed");
    let _ = std::fs::remove_file(&path);
}
