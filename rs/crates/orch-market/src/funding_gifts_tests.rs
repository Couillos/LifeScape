//! Gift sequence through the real deal state machine: convocation of
//! both parties, delivery rows, settlement verified, typed failure,
//! retried once then abandoned, no market trace. Temp files, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_market::DealStatus;

use super::{section_lines, summarize, COINS_ID, GIFT_ID_BASE};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

fn temp(tag: &str, ext: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-funding-{}-{tag}-{stamp}.{ext}", std::process::id()))
}

fn service(tag: &str) -> (MarketService, std::path::PathBuf, std::path::PathBuf) {
    let path = temp(tag, "db");
    let catalogue = vec![CatalogueItem { id: COINS_ID, name: "Coins".to_string(), shop_price: 1, alch_value: 0 }];
    let service = MarketService::open(&path, catalogue).expect("opens");
    let journal = temp(tag, "jsonl");
    service.set_funding_journal(journal.clone());
    (service, path, journal)
}

fn cleanup(paths: &[&std::path::Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn call(service: &MarketService, op: &str, fields: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", str_value(op))];
    all.extend(fields);
    service.handle(&object(all))
}

fn convene(service: &MarketService, deal: u64, sweep_wall: f64) -> Value {
    let entry = object(vec![
        ("deal", Value::Int(deal as i64)),
        ("depart_buyer", Value::Int(30)),
        ("depart_seller", Value::Int(90)),
        ("deadline_buyer", Value::Int(1500)),
        ("deadline_seller", Value::Int(1500)),
        ("sweep_wall", Value::Float(sweep_wall)),
    ]);
    call(service, "convene", vec![("deals", Value::Arr(vec![entry]))])
}

fn status(service: &MarketService, deal: u64) -> DealStatus {
    service.lock_engine().deals.get(&deal).expect("deal").status
}

/// The id the coins carry in the dumps.
#[test]
fn coins_id_is_the_dump_id() {
    let coins = lifescape_gamedata::objs_by_name("coins").into_iter().next().expect("coins in the dump");
    assert_eq!(coins.obj_id as i64, COINS_ID);
}

/// Donor and maker are convened like a deal (both get a pose, adjacent
/// tiles), both see the gift in `my_deals` (coins, price 0), the maker's
/// settled report carries the coins received: journaled ok, no print,
/// no fill, the convocation freed.
#[test]
fn gift_settles_like_a_deal_without_a_market_trace() {
    let (service, db, journal) = service("settle");
    let gift = service.create_gift("rich", "mm_a", 40_000, 1).expect("created");
    assert!(gift > GIFT_ID_BASE);
    assert_eq!(service.gifts_in_flight(), vec![("rich".to_string(), "mm_a".to_string(), 40_000)]);
    assert_eq!((service.gifts_owed("rich"), service.gifts_owed("mm_a")), (40_000, 0), "the donor's coins are committed");
    for account in ["rich", "mm_a"] {
        let rows = call(&service, "my_deals", vec![("account", str_value(account))]);
        let row = rows.get("deals").as_arr().cloned().unwrap_or_default().into_iter().find(|row| row.get("deal").as_i64() == Some(gift as i64)).expect("listed");
        assert_eq!((row.get("item").as_i64(), row.get("qty").as_i64(), row.get("price").as_i64()), (Some(COINS_ID), Some(40_000), Some(0)));
        assert_eq!((row.get("seller").as_str(), row.get("buyer").as_str()), (Some("rich"), Some("mm_a")), "donor gives, maker receives");
    }
    let pending = call(&service, "pending", vec![("now_wall", Value::Float(0.0)), ("tick_ms", Value::Int(60))]);
    assert!(pending.get("deals").as_arr().cloned().unwrap_or_default().iter().any(|row| row.get("deal").as_i64() == Some(gift as i64)), "awaits its convocation");
    let poses = convene(&service, gift, 1e12);
    let accounts: Vec<&str> = poses.get("poses").as_arr().map(|rows| rows.iter().filter_map(|row| row.get("account").as_str()).collect()).unwrap_or_default();
    assert_eq!(accounts, vec!["mm_a", "rich"], "both parties convened: {poses:?}");
    assert_eq!(status(&service, gift), DealStatus::Convened);
    for account in ["rich", "mm_a"] {
        let arrived = call(&service, "deal_arrived", vec![("account", str_value(account)), ("deal", Value::Int(gift as i64))]);
        assert_eq!(arrived.get("ok").as_bool(), Some(true), "{arrived:?}");
    }
    let trading = call(&service, "deal_report", vec![("account", str_value("rich")), ("deal", Value::Int(gift as i64)), ("outcome", str_value("trading"))]);
    assert_eq!(trading.get("ok").as_bool(), Some(true), "{trading:?}");
    let settled = call(
        &service,
        "deal_report",
        vec![("account", str_value("mm_a")), ("deal", Value::Int(gift as i64)), ("outcome", str_value("settled")), ("received", Value::Int(40_000))],
    );
    assert_eq!(settled.get("ok").as_bool(), Some(true), "{settled:?}");
    assert_eq!(status(&service, gift), DealStatus::Settled);
    assert!(service.gifts_in_flight().is_empty());
    assert!(service.lock_engine().convened_accounts.is_empty(), "convocation freed");
    assert!(service.lock_engine().recent.is_empty(), "no live print");
    assert!(service.last_fills(COINS_ID).is_empty(), "no execution");
    let text = std::fs::read_to_string(&journal).expect("journal");
    let summaries = summarize(&text);
    assert_eq!(summaries.len(), 1);
    assert_eq!((summaries[0].state.as_str(), summaries[0].received), ("settled", Some(40_000)));
    assert!(text.contains("received ok"), "{text}");
    let lines = section_lines(&summaries);
    assert!(lines.iter().any(|line| line.contains("1 settled (40000 coins)")), "{lines:#?}");
    cleanup(&[&db, &journal]);
}

/// A gift nobody delivers fails `no_show` at the sweep, is created
/// again once; the retry refused at the window (`mismatch`) is
/// abandoned. Every step journaled, nothing relisted.
#[test]
fn failed_gift_is_retried_once_then_abandoned() {
    let (service, db, journal) = service("fail");
    let first = service.create_gift("rich", "mm_b", 25_000, 1).expect("created");
    convene(&service, first, 1.0);
    let swept = call(&service, "sweep", vec![("now_wall", Value::Float(10.0)), ("nav_up", Value::Bool(true))]);
    assert_eq!(swept.get("failed").as_arr().map(|rows| rows.len()), Some(1), "{swept:?}");
    assert_eq!(service.lock_engine().deals.get(&first).and_then(|deal| deal.fail).map(|why| why.as_str()), Some("no_show"));
    let flying = service.gifts_in_flight();
    assert_eq!(flying, vec![("rich".to_string(), "mm_b".to_string(), 25_000)], "retried once");
    let retry = *service.lock_engine().gifts.gifts.keys().max().expect("retry");
    assert_ne!(retry, first);
    assert_eq!(service.lock_engine().gifts.gifts[&retry].attempt, 2);
    convene(&service, retry, 1e12);
    let refused = call(
        &service,
        "deal_report",
        vec![("account", str_value("mm_b")), ("deal", Value::Int(retry as i64)), ("outcome", str_value("failed")), ("reason", str_value("mismatch"))],
    );
    assert_eq!(refused.get("ok").as_bool(), Some(true), "{refused:?}");
    assert!(service.gifts_in_flight().is_empty(), "no third try");
    assert!(service.lock_engine().convened_accounts.is_empty());
    let orders = service.lock_store().account_orders("rich").unwrap_or_default();
    assert!(orders.is_empty(), "never relisted as an order: {orders:?}");
    let summaries = summarize(&std::fs::read_to_string(&journal).expect("journal"));
    let states: Vec<&str> = summaries.iter().map(|gift| gift.state.as_str()).collect();
    assert_eq!(states.len(), 2);
    assert!(states[0].starts_with("failed no_show, retried as gift"), "{states:?}");
    assert_eq!(states[1], "abandoned mismatch");
    cleanup(&[&db, &journal]);
}

/// Bad terms are typed refusals.
#[test]
fn bad_gift_terms_are_refused() {
    let (service, db, journal) = service("terms");
    assert!(service.create_gift("rich", "rich", 10, 1).is_err());
    assert!(service.create_gift("rich", "mm_a", 0, 1).is_err());
    assert!(service.create_gift("", "mm_a", 10, 1).is_err());
    cleanup(&[&db, &journal]);
}

/// A gift without a terminal line from an older daemon run is lost.
#[test]
fn journal_marks_restart_losses() {
    let line = |boot: i64, gift: i64, event: &str| {
        lifescape_json::stringify(&object(vec![
            ("boot", Value::Int(boot)),
            ("gift", Value::Int(gift)),
            ("donor", str_value("rich")),
            ("maker", str_value("mm_a")),
            ("amount", Value::Int(5_000)),
            ("attempt", Value::Int(1)),
            ("event", str_value(event)),
            ("reason", str_value("")),
        ]))
    };
    let text = [line(1, 7, "created"), line(2, 7, "created")].join("\n");
    let states: Vec<String> = summarize(&text).into_iter().map(|gift| gift.state).collect();
    assert_eq!(states, vec!["lost (daemon restart)".to_string(), "in flight".to_string()]);
}
