#![forbid(unsafe_code)]
//! Crossed book replay (live 2026-11-13, bronze axe 1351): a failed
//! deal relisted lara_lane's bid at 20 above third-party asks at 16
//! without matching, and the book stayed crossed (buys [20,12,4], sells
//! [15,16,16,18,19]). The book is never crossed now, except an account's
//! own bid against its own ask and the two parties of a just-failed deal
//! during their cooldown. Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_market::{DealStatus, Side};

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const BRONZE_AXE: i64 = 1351;

fn catalogue() -> Vec<CatalogueItem> {
    vec![
        CatalogueItem { id: BRONZE_AXE, name: "Bronze axe".to_string(), shop_price: 16, alch_value: 6 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ]
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-market-uncross-{}-{tag}-{stamp}.db", std::process::id()))
}

fn cleanup(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(BRONZE_AXE)),
        ("qty", Value::Int(100)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn place(service: &MarketService, account: &str, side: &str, price: i64) -> Value {
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(BRONZE_AXE)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(price)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{account} {side} @{price}: {reply:?}");
    reply
}

fn fail_deal(service: &MarketService, account: &str, deal: i64, reason: &str) {
    let reply = service.handle(&object(vec![
        ("op", str_value("deal_report")),
        ("account", str_value(account)),
        ("deal", Value::Int(deal)),
        ("outcome", str_value("failed")),
        ("reason", str_value(reason)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "deal {deal} fails: {reply:?}");
}

fn first_deal(reply: &Value) -> i64 {
    reply.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().expect("deal id")
}

/// (buyer, seller, price) of every deal still waiting for its handover.
fn matched_deals(service: &MarketService) -> Vec<(String, String, i64)> {
    let engine = service.lock_engine();
    let mut found: Vec<(u64, String, String, i64)> = engine
        .deals
        .iter()
        .filter(|(_, deal)| deal.status == DealStatus::Matched)
        .map(|(id, deal)| (*id, deal.buyer.clone(), deal.seller.clone(), deal.price))
        .collect();
    found.sort();
    found.into_iter().map(|(_, buyer, seller, price)| (buyer, seller, price)).collect()
}

fn depth(service: &MarketService, side: Side) -> Vec<(i64, i64)> {
    service.lock_engine().books.get(&BRONZE_AXE).map(|book| book.depth(side, 20)).unwrap_or_default()
}

fn stored_deals(service: &MarketService) -> usize {
    service.lock_store().recent_deals(100).expect("reads").len()
}

/// The live sequence: lara_lane's bid at 20 meets bmzvbp5u's ask at 15
/// (deal at 20), asks at 16 arrive while the bid is in flight, the deal
/// times out and both lots relist. The relisted bid re-enters as a
/// taker: it takes the oldest 16 ask at 16 (the resting order's price),
/// never bmzvbp5u again (cooldown), and nothing rests crossed.
#[test]
fn relisted_bid_takes_the_resting_third_party_ask_at_its_price() {
    let path = temp_db("relist-third");
    let service = MarketService::open(&path, catalogue()).expect("opens");
    for account in ["lara_lane", "bmzvbp5u", "b3gybcev", "b5xntyz6"] {
        funded(&service, account);
    }
    place(&service, "lara_lane", "buy", 20);
    let deal = first_deal(&place(&service, "bmzvbp5u", "sell", 15));
    let b3gy_order = place(&service, "b3gybcev", "sell", 16).get("order").as_i64().expect("order");
    place(&service, "b5xntyz6", "sell", 16);
    assert_eq!(depth(&service, Side::Buy), Vec::<(i64, i64)>::new(), "bid in flight");
    fail_deal(&service, "lara_lane", deal, "timeout");
    assert_eq!(
        matched_deals(&service),
        vec![("lara_lane".to_string(), "b3gybcev".to_string(), 16)],
        "relisted bid fills against the oldest third-party ask, at the ask"
    );
    assert_eq!(depth(&service, Side::Buy), Vec::<(i64, i64)>::new(), "no bid left resting");
    assert_eq!(depth(&service, Side::Sell), vec![(15, 1), (16, 1)], "bmzvbp5u and b5xntyz6 still rest");
    let b3gy_row = service.lock_store().get_order(b3gy_order).expect("reads").expect("row");
    assert_eq!(b3gy_row.status, lifescape_store::OrderStatus::Settled, "maker drained through the normal fill path");
    cleanup(&path);
}

/// Self-trade never happens, but an account's own ask must not shield
/// the third-party asks behind it: the bid skips its own order.
#[test]
fn own_ask_is_skipped_not_a_wall() {
    let path = temp_db("self-skip");
    let service = MarketService::open(&path, catalogue()).expect("opens");
    for account in ["solo", "other"] {
        funded(&service, account);
    }
    place(&service, "solo", "sell", 15);
    place(&service, "other", "sell", 16);
    let reply = place(&service, "solo", "buy", 20);
    assert_eq!(reply.get("status").as_str(), Some("filled"), "{reply:?}");
    assert_eq!(matched_deals(&service), vec![("solo".to_string(), "other".to_string(), 16)]);
    assert_eq!(depth(&service, Side::Sell), vec![(15, 1)], "own ask untouched");
    cleanup(&path);
}

/// A carnet left crossed by the old relist heals at boot: the crossed
/// rows match through the normal fill path at the older order's price,
/// the recently failed pair stays apart, and a second boot adds no deal.
#[test]
fn boot_reload_heals_a_crossed_carnet_once() {
    let path = temp_db("boot-heal");
    let now = MarketService::now_clock();
    {
        let service = MarketService::open(&path, catalogue()).expect("opens");
        let store = service.lock_store();
        let row = |account: &str, side: lifescape_store::OrderSide, price: i64, clock: i64| lifescape_store::NewOrder {
            account: account.to_string(),
            side,
            item_id: BRONZE_AXE,
            quantity: 1,
            price,
            dormant: false,
            clock,
        };
        store.place_order(&row("b3gybcev", lifescape_store::OrderSide::Sell, 16, now - 600)).expect("rests");
        // Created before the asks (old rank), re-entered last: the taker.
        let lara = store.place_order(&row("lara_lane", lifescape_store::OrderSide::Buy, 20, now - 900)).expect("rests");
        store.set_order_status(lara, lifescape_store::OrderStatus::Open, 1, now - 60).expect("re-entered");
        store.place_order(&row("bmzvbp5u", lifescape_store::OrderSide::Sell, 15, now - 60)).expect("rests");
        let failed = store
            .create_deal(&lifescape_store::NewDeal {
                buyer: "lara_lane".to_string(),
                seller: "bmzvbp5u".to_string(),
                item_id: BRONZE_AXE,
                quantity: 1,
                price: 20,
                meeting_x: 3211,
                meeting_y: 3427,
                meeting_level: 0,
                deadline_tick: now + 3600,
                clock: now - 120,
                taker_side: lifescape_store::OrderSide::Buy,
            })
            .expect("deal");
        store.set_deal_status(failed, lifescape_store::DealStatus::Failed, "timeout", now - 60).expect("fails");
    }
    let healed = MarketService::open(&path, catalogue()).expect("reopens");
    assert_eq!(
        matched_deals(&healed),
        vec![("lara_lane".to_string(), "b3gybcev".to_string(), 16)],
        "the latest re-entry is the taker: lara pays the resting 16, not her 20"
    );
    assert_eq!(depth(&healed, Side::Buy), Vec::<(i64, i64)>::new());
    assert_eq!(depth(&healed, Side::Sell), vec![(15, 1)], "the failed partner is not matched");
    assert_eq!(stored_deals(&healed), 2, "one failed + one heal deal");
    drop(healed);
    let again = MarketService::open(&path, catalogue()).expect("reopens");
    assert_eq!(stored_deals(&again), 2, "no double deal on the next boot");
    cleanup(&path);
}

fn place_qty(service: &MarketService, account: &str, side: &str, qty: i64, price: i64) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(BRONZE_AXE)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]))
}

/// User rule 2026-11-13: a bid relisted at 20 pays the asks resting
/// under it, best first (15 then 16), never its own 20.
#[test]
fn relisted_bid_pays_the_best_asks_never_its_limit() {
    let path = temp_db("relist-best-asks");
    let service = MarketService::open(&path, catalogue()).expect("opens");
    for account in ["lara_lane", "bmzvbp5u", "third_a", "third_b"] {
        funded(&service, account);
    }
    let bid = place_qty(&service, "lara_lane", "buy", 2, 20);
    assert_eq!(bid.get("status").as_str(), Some("open"), "{bid:?}");
    let deal = first_deal(&place_qty(&service, "bmzvbp5u", "sell", 2, 14));
    place(&service, "third_a", "sell", 16);
    place(&service, "third_b", "sell", 15);
    fail_deal(&service, "bmzvbp5u", deal, "nav_down");
    assert_eq!(
        matched_deals(&service),
        vec![("lara_lane".to_string(), "third_b".to_string(), 15), ("lara_lane".to_string(), "third_a".to_string(), 16)],
        "best ask first, each at its own price"
    );
    assert_eq!(depth(&service, Side::Buy), Vec::<(i64, i64)>::new());
    assert_eq!(depth(&service, Side::Sell), vec![(14, 2)], "the failed partner rests, kept apart");
    cleanup(&path);
}

/// The failed pair rests crossed during its cooldown (no immediate
/// rematch, no deal), matches once it expires, and only once.
#[test]
fn failed_pair_waits_out_its_cooldown_then_matches_once() {
    let path = temp_db("pair-cooldown");
    let service = MarketService::open(&path, catalogue()).expect("opens");
    for account in ["seller", "buyer"] {
        funded(&service, account);
    }
    place(&service, "seller", "sell", 4);
    let deal = first_deal(&place(&service, "buyer", "buy", 5));
    fail_deal(&service, "buyer", deal, "timeout");
    assert!(matched_deals(&service).is_empty(), "no rematch of the failed pair");
    assert_eq!(depth(&service, Side::Buy), vec![(5, 1)]);
    assert_eq!(depth(&service, Side::Sell), vec![(4, 1)]);
    let now = crate::market_convene::wall_now();
    let window = crate::market_uncross::cooldown_secs();
    assert!(service.uncross_books(now + window - 5.0).is_empty(), "still inside the cooldown");
    // A fresh order of the same pair is kept apart too.
    let again = place(&service, "seller", "sell", 5);
    assert_eq!(again.get("status").as_str(), Some("open"), "{again:?}");
    let healed = service.uncross_books(now + window + 5.0);
    assert_eq!(healed.len(), 1, "one deal once the cooldown expired: {healed:?}");
    assert_eq!(matched_deals(&service), vec![("buyer".to_string(), "seller".to_string(), 4)], "the resting ask sets the price");
    assert!(service.uncross_books(now + window + 10.0).is_empty(), "no double deal");
    assert_eq!(depth(&service, Side::Buy), Vec::<(i64, i64)>::new());
    cleanup(&path);
}
