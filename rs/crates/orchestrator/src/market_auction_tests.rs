//! Grouped auction, mode and anchor on the real service (temp database,
//! no daemon): lot 3 marché, B2 to B5.

use lifescape_json::{object, str_value, Value};

use super::{anchor_of, auction_round, in_auction, period_secs};
use crate::market_reprice::reprice_round;
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;
/// 60 ms ticks: an auction period is 360 s, a walk window 2 000 ticks = 120 s.
const TICK_MS: f64 = 60.0;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("lifescape-auction-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 }, CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 }];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

/// A service whose unmeasured objects get their mode at the first
/// order, as in the daemon (the plain-book tests keep it continuous).
fn auction_service(tag: &str) -> (MarketService, std::path::PathBuf) {
    super::enforce_first_order_mode();
    service(tag)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&object(vec![("op", str_value("fund")), ("account", str_value(account)), ("coins", Value::Int(1_000_000)), ("item", Value::Int(FEATHER)), ("qty", Value::Int(10_000))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn place(service: &MarketService, account: &str, side: &str, qty: i64, price: i64, extra: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", str_value("place")), ("account", str_value(account)), ("side", str_value(side)), ("item", Value::Int(FEATHER)), ("qty", Value::Int(qty)), ("price", Value::Int(price))];
    all.extend(extra);
    let reply = service.handle(&object(all));
    assert_eq!(reply.get("ok").as_bool(), Some(true), "{reply:?}");
    reply
}

fn open_price(service: &MarketService, account: &str) -> Option<i64> {
    service.lock_store().account_orders(account).unwrap_or_default().into_iter().find(|row| matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0).map(|row| row.price)
}

fn deals_of(service: &MarketService, account: &str) -> Vec<(i64, i64)> {
    service.lock_store().open_deals_for_account(account).unwrap_or_default().into_iter().map(|deal| (deal.quantity, deal.price)).collect()
}

/// B2 + B3: an object nobody traded goes to auction; its orders rest
/// crossed between two auctions, then every pair that agrees trades at
/// the one price that moves the most units (here the middle of [6, 10]).
#[test]
fn a_rare_object_trades_only_at_its_auction_at_one_price() {
    let (service, path) = auction_service("rare");
    for account in ["seller", "low", "high"] {
        funded(&service, account);
    }
    place(&service, "seller", "sell", 3, 6, vec![]);
    place(&service, "low", "buy", 3, 4, vec![]);
    let start = 1000.0;
    let first = auction_round(&service, start, TICK_MS);
    assert_eq!((first.auctioned, first.entered, first.deals), (1, 0, 0), "in auction since its first order, nothing crosses yet");
    assert!(in_auction(&service, FEATHER));
    // A bid over the ask arrives between two auctions: it rests.
    let crossing = place(&service, "high", "buy", 3, 10, vec![]);
    assert_eq!(crossing.get("status").as_str(), Some("open"), "no match on arrival: {crossing:?}");
    assert!(deals_of(&service, "seller").is_empty());
    assert_eq!(auction_round(&service, start + period_secs(TICK_MS) / 2.0, TICK_MS).deals, 0, "not before the period");
    let second = auction_round(&service, start + period_secs(TICK_MS) + 1.0, TICK_MS);
    assert_eq!(second.deals, 1);
    assert_eq!(deals_of(&service, "seller"), vec![(3, 8)], "one price: the middle of 6 and 10");
    assert_eq!(deals_of(&service, "high"), vec![(3, 8)], "the buyer pays 8, less than its 10");
    assert!(deals_of(&service, "low").is_empty(), "the bid under the price waits");
    cleanup(&path);
}

/// B2: between two auctions the orders do not walk; after a missed
/// auction they walk once, then wait for the next.
#[test]
fn the_walk_plays_only_after_a_missed_auction() {
    let (service, path) = auction_service("walk");
    funded(&service, "buyer");
    funded(&service, "seller");
    place(&service, "seller", "sell", 5, 40, vec![]);
    place(&service, "buyer", "buy", 5, 4, vec![("ceiling", Value::Int(100)), ("patience", Value::Int(1))]);
    let start = 1000.0;
    // The first round puts the object in auction and misses (4 < 40).
    auction_round(&service, start, TICK_MS);
    reprice_round(&service, start, 60.0);
    assert_eq!(open_price(&service, "buyer"), Some(5), "after the missed auction: one 10 % step (at least a coin)");
    // Hours of windows without an auction: the order does not move.
    for hour in 1..=5 {
        reprice_round(&service, start + 3600.0 * f64::from(hour) / 20.0, 60.0);
    }
    assert_eq!(open_price(&service, "buyer"), Some(5), "between two auctions the offers rest");
    auction_round(&service, start + period_secs(TICK_MS) + 1.0, TICK_MS);
    reprice_round(&service, start + period_secs(TICK_MS) + 1.0, 60.0);
    assert_eq!(open_price(&service, "buyer"), Some(6), "the next missed auction: the next step");
    cleanup(&path);
}

/// B3: an object that trades often stays continuous: a crossing order
/// matches on arrival as before.
#[test]
fn a_frequently_traded_object_stays_continuous() {
    let (service, path) = auction_service("brisk");
    funded(&service, "seller");
    funded(&service, "buyer");
    let now = MarketService::now_clock();
    for index in 0..8 {
        service.lock_store().record_execution(&lifescape_store::NewExecution { item_id: FEATHER, quantity: 1, price: 5, clock: now - 10 * (index + 1), server_tick: 0, taker_side: None }).expect("execution");
    }
    place(&service, "seller", "sell", 2, 9, vec![]);
    let report = auction_round(&service, 1000.0, TICK_MS);
    assert_eq!(report.auctioned, 0, "a trade every 10 s (166 ticks) is far under a window");
    assert!(!in_auction(&service, FEATHER));
    let taker = place(&service, "buyer", "buy", 2, 9, vec![]);
    assert_eq!(taker.get("status").as_str(), Some("filled"), "continuous: matched on arrival: {taker:?}");
    cleanup(&path);
}

/// B3 hysteresis on the service: an auction object whose executions are
/// between one and two windows apart stays in auction; once it has
/// auctions behind it, the pairs it served decide (not executions that
/// predate its mode): an auction serving nothing stays an auction.
#[test]
fn an_auction_object_stays_between_the_thresholds_and_while_it_serves_nothing() {
    let (service, path) = auction_service("hysteresis");
    funded(&service, "seller");
    place(&service, "seller", "sell", 2, 9, vec![]);
    auction_round(&service, 1000.0, TICK_MS);
    assert!(in_auction(&service, FEATHER));
    let now = MarketService::now_clock();
    // 90 s apart = 1 500 ticks: between 1 000 and 2 000.
    for index in 0..8 {
        service.lock_store().record_execution(&lifescape_store::NewExecution { item_id: FEATHER, quantity: 1, price: 5, clock: now - 5 - 90 * index, server_tick: 0, taker_side: None }).expect("execution");
    }
    auction_round(&service, 1000.0 + period_secs(TICK_MS) + 1.0, TICK_MS);
    assert!(in_auction(&service, FEATHER), "between the thresholds: no flip");
    for index in 0..8 {
        service.lock_store().record_execution(&lifescape_store::NewExecution { item_id: FEATHER, quantity: 1, price: 5, clock: now - 1 - 20 * index, server_tick: 0, taker_side: None }).expect("execution");
    }
    let later = auction_round(&service, 1000.0 + 2.0 * period_secs(TICK_MS) + 2.0, TICK_MS);
    assert_eq!(later.left, 0, "two auctions served no pair: the executions of another mode do not bring it back");
    assert!(in_auction(&service, FEATHER));
    cleanup(&path);
}

/// B4: an isolated one-coin sale hardly moves the anchor, and the asking
/// account's own fills never count (the exclusion is in the query, so
/// the limit of 50 counts only the others).
#[test]
fn the_anchor_is_volume_weighted_and_leaves_out_own_fills() {
    let (service, path) = service("anchor_fn");
    for account in ["s1", "b1", "s2", "b2"] {
        funded(&service, account);
    }
    place(&service, "s1", "sell", 10, 50, vec![]);
    place(&service, "b1", "buy", 10, 50, vec![]);
    place(&service, "s2", "sell", 1, 1, vec![]);
    place(&service, "b2", "buy", 1, 1, vec![]);
    crate::market_test_support::settle_matched(&service);
    assert_eq!(anchor_of(&service, FEATHER, "watcher", TICK_MS), Some(46), "(500 + 1) / 11, not the last price 1");
    assert_eq!(anchor_of(&service, FEATHER, "s1", TICK_MS), Some(1), "s1's own sale never anchors its quote");
    assert_eq!(anchor_of(&service, 995, "watcher", TICK_MS), None, "no trade: no anchor");
    cleanup(&path);
}

/// B4 + B5 on the service: with trades at 50 (ten units) and an isolated
/// one at 1, a bid resting at 46, the volume-weighted mean, climbs one
/// percent (a coin), not ten percent toward the last price.
#[test]
fn a_bid_on_the_mean_price_steps_a_coin_not_ten_percent() {
    let (service, path) = service("anchor");
    for account in ["s1", "b1", "s2", "b2", "watcher"] {
        funded(&service, account);
    }
    place(&service, "s1", "sell", 10, 50, vec![]);
    place(&service, "b1", "buy", 10, 50, vec![]);
    place(&service, "s2", "sell", 1, 1, vec![]);
    place(&service, "b2", "buy", 1, 1, vec![]);
    crate::market_test_support::settle_matched(&service);
    place(&service, "watcher", "buy", 3, 46, vec![("ceiling", Value::Int(500)), ("patience", Value::Int(1))]);
    let now = MarketService::now_clock() as f64;
    reprice_round(&service, now + 10.0, TICK_MS);
    assert_eq!(open_price(&service, "watcher"), Some(47), "1 % of 46 rounds up to a coin; 10 % would be 51");
    cleanup(&path);
}

/// Review lot 3 (B3): an auction object trades once per auction, so its
/// executions alone could never bring it back. Six pairs served per
/// auction, over two auctions, is one trade per 1 000 ticks: continuous.
#[test]
fn an_auction_object_that_serves_enough_pairs_returns_to_continuous() {
    let (service, path) = auction_service("served");
    let accounts: Vec<String> = (0..6).flat_map(|index| [format!("buyer{index}"), format!("seller{index}")]).collect();
    for account in &accounts {
        funded(&service, account);
    }
    let start = 1000.0;
    let mut left = 0;
    for round in 0..3 {
        for index in 0..6 {
            place(&service, &format!("seller{index}"), "sell", 1, 5, vec![]);
            place(&service, &format!("buyer{index}"), "buy", 1, 10, vec![]);
        }
        let report = auction_round(&service, start + f64::from(round) * (period_secs(TICK_MS) + 1.0), TICK_MS);
        left += report.left;
        if round == 0 {
            assert_eq!(report.deals, 6, "the first auction serves the six pairs: {report:?}");
        }
    }
    assert_eq!(left, 1, "served 6 pairs in each of two auctions: back to continuous");
    assert!(!in_auction(&service, FEATHER));
    cleanup(&path);
}

/// Review lot 3 (2): a pressed order (a blocking need, an urgent sale)
/// never waits for the period: it triggers an auction on the object at
/// once, same single price for every order present.
#[test]
fn a_need_bid_is_served_at_once_on_an_auction_object() {
    let (service, path) = auction_service("pressed");
    for account in ["seller", "low", "fighter"] {
        funded(&service, account);
    }
    place(&service, "seller", "sell", 2, 5, vec![]);
    place(&service, "low", "buy", 2, 4, vec![]);
    auction_round(&service, 1000.0, TICK_MS);
    assert!(in_auction(&service, FEATHER));
    let need = place(&service, "fighter", "buy", 2, 10, vec![("motive", str_value("need")), ("ceiling", Value::Int(20))]);
    assert_eq!(need.get("status").as_str(), Some("filled"), "served at once: {need:?}");
    assert_eq!(deals_of(&service, "fighter"), vec![(2, 7)], "one price for all: the middle of 5 and 10");
    assert!(deals_of(&service, "low").is_empty());
    cleanup(&path);
}

#[test]
fn an_urgent_sale_is_served_at_once_and_a_gear_bid_waits() {
    let (service, path) = auction_service("urgent");
    for account in ["buyer", "seller", "low", "gear"] {
        funded(&service, account);
    }
    place(&service, "buyer", "buy", 2, 9, vec![]);
    place(&service, "low", "sell", 2, 40, vec![]);
    auction_round(&service, 1000.0, TICK_MS);
    let waits = place(&service, "gear", "buy", 2, 41, vec![("motive", str_value("gear")), ("ceiling", Value::Int(60))]);
    assert_eq!(waits.get("status").as_str(), Some("open"), "{waits:?}");
    assert!(deals_of(&service, "gear").is_empty(), "a gear bid waits for the period");
    let dump = place(&service, "seller", "sell", 2, 3, vec![("urgent", Value::Bool(true))]);
    assert_eq!(dump.get("status").as_str(), Some("filled"), "{dump:?}");
    cleanup(&path);
}

/// Review lot 3 (6): an object traded fewer than twice is in auction
/// mode from its first order: no continuous match before the next round.
#[test]
fn the_first_orders_of_a_never_traded_object_rest_for_the_auction() {
    let (service, path) = auction_service("first");
    funded(&service, "seller");
    funded(&service, "buyer");
    place(&service, "seller", "sell", 1, 5, vec![]);
    let bid = place(&service, "buyer", "buy", 1, 9, vec![]);
    assert_eq!(bid.get("status").as_str(), Some("open"), "no continuous match: {bid:?}");
    assert!(in_auction(&service, FEATHER));
    cleanup(&path);
}
