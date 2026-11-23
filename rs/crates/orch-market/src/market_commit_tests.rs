#![forbid(unsafe_code)]
//! Live 2026-11-13 (21:19-21:58 CET): every `short` deal was stock or
//! coins promised twice. A sell fully matched into a deal leaves no
//! resting row, yet its goods sit in the bank until the handover; the
//! ledger re-mirrored them from the live snapshot every round, so the
//! brain listed them again and the relist matched at once (bct5rxpg
//! 1716 + 1722 for 12 arrows held, fiona_park 1698 + 1721, 1692 + 1720).
//! Bids did the same with coins (bd23dqpq 1643-1652, bct5rxpg 1772).
//! Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const ARROW: i64 = 882;
const COINS: i64 = 995;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("lifescape-market-commit-{}-{tag}-{stamp}.db", std::process::id()));
    let catalogue = vec![
        CatalogueItem { id: ARROW, name: "Bronze arrow".to_string(), shop_price: 1, alch_value: 0 },
        CatalogueItem { id: COINS, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn place(service: &MarketService, account: &str, side: &str, qty: i64, price: i64) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(ARROW)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]))
}

/// bct5rxpg: 12 arrows held. A resting listing claims its 7; a matched
/// one keeps them promised after the next ledger mirror (the snapshot
/// still shows them in the bank): the third listing is refused.
#[test]
fn promised_stock_is_never_listed_twice() {
    let (service, path) = service("stock");
    service.sync_holdings("bct5rxpg", 0, &[(ARROW, 12)]);
    service.sync_holdings("b5xntyz6", 1000, &[]);
    assert_eq!(place(&service, "bct5rxpg", "sell", 7, 1).get("ok").as_bool(), Some(true), "7 of 12");
    let over = place(&service, "bct5rxpg", "sell", 7, 2);
    assert_eq!(over.get("ok").as_bool(), Some(false), "5 free, 7 asked: {over:?}");
    let bought = place(&service, "b5xntyz6", "buy", 7, 1);
    assert_eq!(bought.get("deals").as_arr().map(|deals| deals.len()), Some(1), "deal 1716: {bought:?}");
    // Next convene round: the live snapshot still holds all 12 arrows.
    service.sync_holdings("bct5rxpg", 7, &[(ARROW, 12)]);
    assert_eq!(place(&service, "bct5rxpg", "sell", 5, 2).get("ok").as_bool(), Some(true), "the 5 really free");
    let relist = place(&service, "bct5rxpg", "sell", 7, 2);
    assert_eq!(relist.get("ok").as_bool(), Some(false), "deal 1722: the promised 7 never list again: {relist:?}");
    cleanup(&path);
}

/// bd23dqpq: a standing bid swept 98 runes for 98 coins, then a second
/// bid of 97 went up on the same purse. Coins promised to in-flight deals
/// and to resting bids are not free.
#[test]
fn promised_coins_are_never_bid_twice() {
    let (service, path) = service("coins");
    service.sync_holdings("seller", 0, &[(ARROW, 500)]);
    service.sync_holdings("bd23dqpq", 172, &[]);
    assert_eq!(place(&service, "seller", "sell", 98, 1).get("ok").as_bool(), Some(true));
    let swept = place(&service, "bd23dqpq", "buy", 98, 1);
    assert_eq!(swept.get("deals").as_arr().map(|deals| deals.len()), Some(1), "{swept:?}");
    // Next round: the coins are still in the bank, the deal not handed over.
    service.sync_holdings("bd23dqpq", 172, &[]);
    let again = place(&service, "bd23dqpq", "buy", 97, 1);
    assert_eq!(again.get("ok").as_bool(), Some(false), "74 free, 97 asked: {again:?}");
    assert_eq!(place(&service, "bd23dqpq", "buy", 50, 1).get("ok").as_bool(), Some(true), "50 of the 74 rest");
    let third = place(&service, "bd23dqpq", "buy", 30, 1);
    assert_eq!(third.get("ok").as_bool(), Some(false), "24 left after the resting 50: {third:?}");
    cleanup(&path);
}
