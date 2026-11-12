#![forbid(unsafe_code)]
//! Offline replay of the live 2026-11-12 deadlock: sellers listed their
//! surplus at 1 coin, buyers needed exactly those goods (a cook its raw
//! shrimps, a firemaker its logs), yet nothing ever crossed. Every bot
//! banks its coins and stock, the server stops transmitting the bank at
//! each close, so the live snapshot showed an empty bank: the brain read
//! 0 coins (no bid at all) and the ledger refused any bid it would have
//! placed. With the closed bank kept in the snapshot, the same book
//! crosses, prints an execution, paints candles and opens a deal.
//! Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};

use crate::market_seed::CatalogueItem;
use crate::market_service::{ledger_holdings, MarketService};

const RAW_SHRIMP: i64 = 317;
const LOGS: i64 = 1511;
const COINS: i64 = 995;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("lifescape-market-cross-{}-{tag}-{stamp}.db", std::process::id()));
    let catalogue = vec![
        CatalogueItem { id: RAW_SHRIMP, name: "Raw shrimps".to_string(), shop_price: 5, alch_value: 2 },
        CatalogueItem { id: LOGS, name: "Logs".to_string(), shop_price: 4, alch_value: 1 },
        CatalogueItem { id: COINS, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

fn row(id: i64, count: i64) -> Value {
    object(vec![("id", Value::Int(id)), ("count", Value::Int(count))])
}

/// Live snapshot shape: pack rows plus the bank rows the heartbeat now
/// keeps once the bank is closed (empty before the fix).
fn snapshot(pack: Vec<Value>, bank: Vec<Value>) -> Value {
    object(vec![("inv", Value::Arr(pack)), ("bank", Value::Arr(bank)), ("worn", Value::Arr(Vec::new()))])
}

fn sync(service: &MarketService, account: &str, live: &Value) {
    let (coins, stock) = ledger_holdings(live, COINS);
    service.lock_engine().ledger.sync_account(account, coins, &stock);
}

fn place(service: &MarketService, account: &str, side: &str, item: i64, qty: i64, price: i64, bound: (&str, i64)) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
        (bound.0, Value::Int(bound.1)),
    ]))
}

#[test]
fn banked_holdings_fund_the_ledger() {
    let live = snapshot(vec![row(RAW_SHRIMP, 3)], vec![row(COINS, 344), row(RAW_SHRIMP, 24), row(LOGS, 22)]);
    let (coins, stock) = ledger_holdings(&live, COINS);
    assert_eq!(coins, 344, "banked coins count");
    assert_eq!(stock, vec![(RAW_SHRIMP, 27), (LOGS, 22)], "pack + bank per object");
}

/// The exact live book: a fisher's 27 raw shrimps listed at 1 (untraded
/// reference), a cook short of raw shrimps with 344 banked coins, a
/// woodcutter's 22 logs and a firemaker with 174 banked coins.
#[test]
fn surplus_listings_cross_the_inputs_buyers_need() {
    let (service, path) = service("live-book");
    // Seller side: stock sits in the bank (brain_sales lists pack + bank).
    sync(&service, "bct5rxpg", &snapshot(Vec::new(), vec![row(RAW_SHRIMP, 27)]));
    sync(&service, "bb4v5d9m", &snapshot(Vec::new(), vec![row(LOGS, 22)]));
    let listed = place(&service, "bct5rxpg", "sell", RAW_SHRIMP, 27, 1, ("floor", 1));
    assert_eq!(listed.get("status").as_str(), Some("open"), "{listed:?}");
    assert_eq!(place(&service, "bb4v5d9m", "sell", LOGS, 22, 1, ("floor", 1)).get("ok").as_bool(), Some(true));

    // Before the fix: the closed bank read empty, the cook held 0 coins
    // and the bid was refused (the brain did not even try: ctx coins=0).
    sync(&service, "bd23dqpq", &snapshot(Vec::new(), Vec::new()));
    let refused = place(&service, "bd23dqpq", "buy", RAW_SHRIMP, 27, 1, ("ceiling", 1));
    assert_eq!(refused.get("ok").as_bool(), Some(false), "no coins seen, no bid: {refused:?}");

    // After: the last known bank carries the coins; the obtain bid takes
    // the covered ask at once (brain_market::place_obtain_bid rule).
    sync(&service, "bd23dqpq", &snapshot(Vec::new(), vec![row(COINS, 344)]));
    let mut ctx = lifescape_planner::expand::ReadyCtx::blind(0.5);
    ctx.coins = 344;
    ctx.price = 1;
    let need = lifescape_planner::INPUT_BATCH;
    let ceiling = lifescape_planner::resolve_obtain::bid_ceiling(&ctx, need);
    assert!(ceiling >= 1, "344 coins cover 27 shrimps at 1");
    let bought = place(&service, "bd23dqpq", "buy", RAW_SHRIMP, need, 1, ("ceiling", ceiling));
    assert_eq!(bought.get("status").as_str(), Some("filled"), "{bought:?}");
    assert_eq!(bought.get("deals").as_arr().map(|deals| deals.len()), Some(1));

    // The firemaker's logs cross the same way.
    sync(&service, "bct5rxpg-fire", &snapshot(vec![row(COINS, 4)], vec![row(COINS, 170)]));
    let logs = place(&service, "bct5rxpg-fire", "buy", LOGS, 22, 1, ("ceiling", 7));
    assert_eq!(logs.get("status").as_str(), Some("filled"), "{logs:?}");

    // Executions print, candles paint (both resolutions), deals wait for
    // their convocation, the ledger moved.
    for (item, volume) in [(RAW_SHRIMP, 27), (LOGS, 22)] {
        for res in ["1h", "1d"] {
            let history = service.handle(&object(vec![("op", str_value("history")), ("item", Value::Int(item)), ("res", str_value(res))]));
            let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
            assert_eq!(candles.len(), 1, "{item} {res}: {candles:?}");
            assert_eq!(candles[0].get("v").as_i64(), Some(volume));
            assert_eq!(candles[0].get("c").as_i64(), Some(1));
        }
        let quote = service.handle(&object(vec![("op", str_value("quote")), ("item", Value::Int(item))]));
        assert_eq!(quote.get("traded").as_bool(), Some(true), "a real print now anchors {item}");
    }
    let pending = |wall: f64| {
        let reply = service.handle(&object(vec![("op", str_value("pending")), ("now_wall", Value::Float(wall)), ("tick_ms", Value::Int(600))]));
        reply.get("deals").as_arr().cloned().unwrap_or_default()
    };
    let first_seen = pending(1.0e9);
    assert_eq!(first_seen.len(), 2, "both deals wait for the handover: {first_seen:?}");
    let later = pending(1.0e9 + crate::market_convene::grouping_secs(600) + 1.0);
    assert!(later.iter().all(|deal| deal.get("convenable").as_bool() == Some(true)), "grouping window over: {later:?}");
    // The convocation hands each party its own tile and departure.
    let payload = |deal: &Value| {
        object(vec![
            ("deal", deal.get("deal").clone()),
            ("depart_buyer", Value::Int(130)),
            ("depart_seller", Value::Int(130)),
            ("deadline_buyer", Value::Int(4000)),
            ("deadline_seller", Value::Int(4000)),
            ("sweep_wall", Value::Float(2.0e9)),
        ])
    };
    let convened = service.handle(&object(vec![("op", str_value("convene")), ("deals", Value::Arr(later.iter().map(payload).collect()))]));
    let posed: Vec<String> =
        convened.get("poses").as_arr().cloned().unwrap_or_default().iter().filter_map(|pose| pose.get("account").as_str().map(str::to_string)).collect();
    for party in ["bd23dqpq", "bct5rxpg", "bb4v5d9m", "bct5rxpg-fire"] {
        assert!(posed.iter().any(|account| account == party), "{party} convened: {convened:?}");
    }
    {
        let engine = service.lock_engine();
        assert_eq!(engine.ledger.coins_of("bd23dqpq"), 344 - 27);
        assert_eq!(engine.ledger.stock_of("bd23dqpq", RAW_SHRIMP), 27);
        assert_eq!(engine.ledger.coins_of("bct5rxpg"), 27);
        assert_eq!(engine.matched, 2);
    }
    cleanup(&path);
}
