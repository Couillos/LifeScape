#![forbid(unsafe_code)]
//! Deal lifecycle tests (user rule 2026-11-13: trades must always end):
//! a deal in `trading` never sticks, a convocation stays held while any
//! of its deals is in flight, an absent party fails its deals and loses
//! its orders while the honest side relists. Temp store, no daemon.

use lifescape_json::{object, str_value, Value};
use lifescape_market::DealStatus;

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

fn fresh_service(tag: &str) -> (MarketService, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("lifescape-convene-life-{}-{tag}-{stamp}.db", std::process::id()));
    let items = vec![
        CatalogueItem { id: 314, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, items).expect("opens"), path)
}

fn cleanup(path: &std::path::PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

/// One sell resting, one buy crossing it: returns the deal id.
fn matched(service: &MarketService, seller: &str, buyer: &str, qty: i64, price: i64) -> i64 {
    for (account, side, limit) in [(seller, "sell", price), (buyer, "buy", price + 1)] {
        let reply = service.handle(&object(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value(side)),
            ("item", Value::Int(314)),
            ("qty", Value::Int(qty)),
            ("price", Value::Int(limit)),
        ]));
        assert_eq!(reply.get("ok").as_bool(), Some(true), "{side} placed: {reply:?}");
        if side == "buy" {
            return reply.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().expect("deal id");
        }
    }
    unreachable!("buy leg returns")
}

fn convene(service: &MarketService, deals: &[i64], sweep_wall: f64) -> Value {
    let payloads: Vec<Value> = deals
        .iter()
        .map(|deal| {
            object(vec![
                ("deal", Value::Int(*deal)),
                ("depart_buyer", Value::Int(1030)),
                ("depart_seller", Value::Int(1030)),
                ("deadline_buyer", Value::Int(9000)),
                ("deadline_seller", Value::Int(9000)),
                ("sweep_wall", Value::Float(sweep_wall)),
            ])
        })
        .collect();
    service.handle(&object(vec![("op", str_value("convene")), ("deals", Value::Arr(payloads))]))
}

fn arrive(service: &MarketService, account: &str, deal: i64) {
    service.handle(&object(vec![("op", str_value("deal_arrived")), ("account", str_value(account)), ("deal", Value::Int(deal))]));
}

fn report(service: &MarketService, account: &str, deal: i64, outcome: &str, reason: &str) -> Value {
    service.handle(&object(vec![
        ("op", str_value("deal_report")),
        ("account", str_value(account)),
        ("deal", Value::Int(deal)),
        ("outcome", str_value(outcome)),
        ("reason", str_value(reason)),
    ]))
}

fn status_of(service: &MarketService, deal: i64) -> DealStatus {
    service.lock_engine().deals.get(&(deal as u64)).expect("deal known").status
}

fn open_orders(service: &MarketService, account: &str) -> usize {
    let reply = service.handle(&object(vec![("op", str_value("my_orders")), ("account", str_value(account))]));
    reply.get("orders").as_arr().cloned().unwrap_or_default().iter().filter(|row| row.get("status").as_str() == Some("open")).count()
}

/// Finding 2: a worker that reported `trading` then crashed never
/// reports again. The sweep closes the deal (timeout, both relist) and
/// frees both accounts for a new convocation.
#[test]
fn sweep_closes_a_stuck_trading_deal() {
    let (service, path) = fresh_service("trading-sweep");
    funded(&service, "seller");
    funded(&service, "buyer");
    let deal = matched(&service, "seller", "buyer", 5, 4);
    convene(&service, &[deal], 5000.0);
    arrive(&service, "buyer", deal);
    arrive(&service, "seller", deal);
    assert_eq!(report(&service, "buyer", deal, "trading", "").get("ok").as_bool(), Some(true));
    assert_eq!(status_of(&service, deal), DealStatus::Trading);
    let swept = service.handle(&object(vec![
        ("op", str_value("sweep")),
        ("now_wall", Value::Float(5001.0)),
        ("nav_up", Value::Bool(true)),
    ]));
    assert_eq!(swept.get("failed").as_arr().map(|rows| rows.len()), Some(1), "stuck trading deal swept: {swept:?}");
    assert_eq!(status_of(&service, deal), DealStatus::Failed);
    assert!(service.lock_engine().convened_accounts.is_empty(), "both accounts free again");
    assert_eq!(open_orders(&service, "buyer"), 1, "neutral timeout relists the buyer");
    assert_eq!(open_orders(&service, "seller"), 1, "neutral timeout relists the seller");
    cleanup(&path);
}

/// Finding 4: the buyer holds two deals in one convocation. The first
/// one ending must not free the buyer while the second is in flight,
/// or a new convocation overwrites the pending pose on the worker.
#[test]
fn convocation_held_while_any_deal_in_flight() {
    let (service, path) = fresh_service("release");
    service.lock_engine().ring = crate::market_convene::build_ring(&|_, _| true);
    for account in ["seller-a", "seller-b", "seller-c", "buyer"] {
        funded(&service, account);
    }
    let first = matched(&service, "seller-a", "buyer", 5, 4);
    let second = matched(&service, "seller-b", "buyer", 6, 4);
    convene(&service, &[first, second], 9000.0);
    arrive(&service, "buyer", first);
    arrive(&service, "seller-a", first);
    report(&service, "buyer", first, "trading", "");
    assert_eq!(report(&service, "buyer", first, "settled", "").get("ok").as_bool(), Some(true));
    assert!(
        service.lock_engine().convened_accounts.contains("buyer"),
        "buyer still convened: deal {second} is in flight"
    );
    assert!(!service.lock_engine().convened_accounts.contains("seller-a"), "seller-a has nothing left");
    // A third match on the buyer must wait for the pending convocation.
    let third = matched(&service, "seller-c", "buyer", 7, 4);
    let reply = convene(&service, &[third], 9000.0);
    assert!(reply.get("poses").as_arr().cloned().unwrap_or_default().is_empty(), "no overwrite: {reply:?}");
    assert_eq!(status_of(&service, third), DealStatus::Matched);
    // The second deal ends: now the buyer is free.
    assert_eq!(report(&service, "seller-b", second, "failed", "timeout").get("ok").as_bool(), Some(true));
    assert!(!service.lock_engine().convened_accounts.contains("buyer"), "buyer free once nothing is in flight");
    cleanup(&path);
}

/// Finding 3: a party that cannot be brought online fails its matched
/// deals as no-show; the honest side relists with its priority, the
/// absent party's open orders are cancelled (they would match and fail
/// again forever).
#[test]
fn absent_party_fails_matched_deals_and_loses_orders() {
    let (service, path) = fresh_service("absent");
    funded(&service, "honest");
    funded(&service, "offline");
    let deal = matched(&service, "offline", "honest", 5, 4);
    // The offline seller also rests another ask on the carnet.
    let extra = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value("offline")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(3)),
        ("price", Value::Int(50)),
    ]));
    assert_eq!(extra.get("ok").as_bool(), Some(true));
    assert_eq!(open_orders(&service, "offline"), 1);
    let failed = service.fail_absent_party("offline");
    assert_eq!(failed, vec![deal], "the matched deal fails");
    assert_eq!(status_of(&service, deal), DealStatus::Failed);
    assert_eq!(
        service.lock_engine().deals.get(&(deal as u64)).and_then(|row| row.fail),
        Some(lifescape_market::FailReason::NoShow)
    );
    assert_eq!(open_orders(&service, "offline"), 0, "absent party's orders cancelled");
    assert_eq!(open_orders(&service, "honest"), 1, "honest side relisted");
    cleanup(&path);
}
