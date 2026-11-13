#![forbid(unsafe_code)]
//! Market history coherence (user rule 2026-11-13: "des bougies
//! cohérentes, pas de trou"): candles run without gap up to the live
//! window, and the trade feeds agree with the execution log after a
//! daemon restart. Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_store::NewExecution;

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;

fn catalogue() -> Vec<CatalogueItem> {
    vec![CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 }]
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-market-history-{}-{tag}-{stamp}.db", std::process::id()))
}

fn cleanup(path: &std::path::PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

fn op(service: &MarketService, pairs: Vec<(&str, Value)>) -> Value {
    let reply = service.handle(&object(pairs));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{reply:?}");
    reply
}

/// An item last traded long ago still draws its price: the close before
/// the window opens it, and flat candles run up to the live hour (the
/// window the front stretches with live prints), each at that close.
#[test]
fn history_runs_flat_up_to_the_live_window() {
    let path = temp_db("live-edge");
    let service = MarketService::open(&path, catalogue()).expect("opens");
    let old_clock = 48_000 * 3600 + 60;
    service
        .lock_store()
        .record_execution(&NewExecution {
            item_id: FEATHER,
            quantity: 3,
            price: 17,
            clock: old_clock,
            server_tick: old_clock,
            taker_side: None,
        })
        .expect("prints");
    let now = MarketService::now_clock();
    for (res, width) in [("1h", 3600), ("1d", 86400)] {
        let history = op(&service, vec![("op", str_value("history")), ("item", Value::Int(FEATHER)), ("res", str_value(res))]);
        assert_eq!(history.get("prev_close").as_i64(), Some(17), "{res}");
        let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
        assert!(!candles.is_empty(), "{res}: the known price still draws");
        let last = candles.last().map(|row| row.get("t").as_i64().unwrap_or(-1)).unwrap_or(-1);
        assert!(last >= (now / width) * width, "{res}: flats reach the live window, last {last} now {now}");
        for row in &candles {
            let ohlc = ["o", "h", "l", "c"].map(|key| row.get(key).as_i64());
            assert_eq!(ohlc, [Some(17); 4], "{res}: flat at the last close: {row:?}");
            assert_eq!(row.get("v").as_i64(), Some(0));
        }
    }
    cleanup(&path);
}

/// The `trades` op (and the WS live tail, `tick`) read the in-memory
/// print ring; after a restart it held only the prints settled since,
/// while `history` (executions) held them all (live 2026-11-13, Bronze
/// axe: 8 executions, 1 print). The ring reloads from the execution log.
#[test]
fn trades_feed_survives_a_restart() {
    let path = temp_db("restart-prints");
    {
        let service = MarketService::open(&path, catalogue()).expect("opens");
        for account in ["maker", "taker"] {
            op(&service, vec![
                ("op", str_value("fund")),
                ("account", str_value(account)),
                ("coins", Value::Int(1_000_000)),
                ("item", Value::Int(FEATHER)),
                ("qty", Value::Int(10_000)),
            ]);
        }
        for price in [16, 12, 20] {
            op(&service, vec![
                ("op", str_value("place")),
                ("account", str_value("maker")),
                ("side", str_value("sell")),
                ("item", Value::Int(FEATHER)),
                ("qty", Value::Int(1)),
                ("price", Value::Int(price)),
            ]);
            op(&service, vec![
                ("op", str_value("place")),
                ("account", str_value("taker")),
                ("side", str_value("buy")),
                ("item", Value::Int(FEATHER)),
                ("qty", Value::Int(1)),
                ("price", Value::Int(price)),
            ]);
            assert_eq!(crate::market_test_support::settle_matched(&service), 1, "one handover at {price}");
        }
    }
    let reopened = MarketService::open(&path, catalogue()).expect("reopens");
    let trades = op(&reopened, vec![("op", str_value("trades")), ("item", Value::Int(FEATHER)), ("limit", Value::Int(30))]);
    let prices: Vec<i64> = trades
        .get("trades")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|row| row.get("p").as_i64())
        .collect();
    assert_eq!(prices, vec![16, 12, 20], "oldest first, like before the restart");
    let tick = op(&reopened, vec![("op", str_value("tick")), ("since", Value::Int(0))]);
    assert_eq!(tick.get("trades").as_arr().map(|rows| rows.len()), Some(3));
    cleanup(&path);
}
