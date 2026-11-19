//! A standing need bid follows the bounds and the need of now (plan
//! équilibre step 4, review lot 4): real service, temp database, no
//! daemon.

use lifescape_json::{object, str_value, Value};

use super::{keep_need_bid, refresh_need_bid, refusal_is_new};
use crate::market_reprice::StandingBid;
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("lifescape-refresh-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![
        CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn funded_with(service: &MarketService, account: &str, coins: i64) {
    let reply = service.handle(&object(vec![("op", str_value("fund")), ("account", str_value(account)), ("coins", Value::Int(coins))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

/// The account's resting buy rows.
fn resting(service: &MarketService, account: &str) -> Vec<lifescape_store::Order> {
    service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0)
        .collect()
}

/// A need bid of 10 placed at ceiling 1 (a reserve of the old time
/// value): the refresh raises it at once to the opening of now, the
/// walk's ceiling follows, the lineage goes on; a refresh with nothing
/// new moves nothing (and logs nothing); over a lower ceiling, down to it.
#[test]
fn a_standing_need_bid_follows_the_bounds_of_now() {
    let (market, path) = service("bounds");
    funded_with(&market, "smith", 10_000);
    let first = keep_need_bid(&market, "smith", FEATHER, 10, 1, 1).expect("placed");
    let StandingBid::Placed { order: root, price: 1 } = first else { panic!("{first:?}") };
    let raised = refresh_need_bid(&market, "smith", FEATHER, 10, 5, 8).expect("refreshed").expect("resting");
    assert_eq!((raised.price, raised.old_price, raised.ceiling, raised.old_ceiling), (5, 1, 8, Some(1)), "{raised:?}");
    assert!(raised.changed());
    let row = market.lock_store().get_order(raised.order).ok().flatten().expect("row");
    assert_eq!((row.root_id, row.motive.as_str(), row.price), (root, "need", 5), "the lineage goes on");
    assert_eq!(market.lock_engine().quote_bounds.get(&raised.order).map(|bounds| bounds.ceiling), Some(8));
    let same = refresh_need_bid(&market, "smith", FEATHER, 10, 5, 8).expect("refreshed").expect("resting");
    assert!(!same.changed(), "nothing moved: nothing to log: {same:?}");
    let lowered = refresh_need_bid(&market, "smith", FEATHER, 10, 1, 3).expect("refreshed").expect("resting");
    assert_eq!((lowered.price, lowered.ceiling), (3, 3), "over the ceiling: down to it");
    assert_eq!(resting(&market, "smith").len(), 1, "never a second bid");
    cleanup(&path);
}

/// Review lot 4, point 1 (live: nathan_t's 2 758-coal bid kept its
/// remainder while it mined 588, it could fill 3 346 for a need of
/// 2 760): the refresh cuts the remainder to the need of now, never
/// raises it.
#[test]
fn a_refreshed_need_bid_never_buys_past_the_need_of_now() {
    let (market, path) = service("trim");
    funded_with(&market, "miner", 10_000);
    keep_need_bid(&market, "miner", FEATHER, 10, 1, 5).expect("placed");
    let trimmed = refresh_need_bid(&market, "miner", FEATHER, 4, 1, 5).expect("refreshed").expect("resting");
    assert_eq!((trimmed.remaining, trimmed.old_remaining), (4, 10), "{trimmed:?}");
    assert_eq!(resting(&market, "miner").iter().map(|row| row.remaining).sum::<i64>(), 4);
    let kept = refresh_need_bid(&market, "miner", FEATHER, 20, 1, 5).expect("refreshed").expect("resting");
    assert_eq!(kept.remaining, 4, "a larger need never raises a resting remainder");
    cleanup(&path);
}

/// A raise the free coins do not cover is refused: the bid rests
/// untouched, the refusal is logged once (not on every launch).
#[test]
fn an_uncovered_raise_is_refused_and_logged_once() {
    let (market, path) = service("uncovered");
    funded_with(&market, "poor", 10);
    keep_need_bid(&market, "poor", FEATHER, 10, 1, 1).expect("placed");
    let refused = refresh_need_bid(&market, "poor", FEATHER, 10, 50, 50).expect_err("the raise needs 490 coins");
    assert_eq!(resting(&market, "poor").iter().map(|row| row.price).collect::<Vec<_>>(), vec![1], "the bid rests untouched");
    assert!(refusal_is_new("poor", FEATHER, &refused), "first refusal: logged");
    assert!(!refusal_is_new("poor", FEATHER, &refused), "the same refusal again: silent");
    assert!(refusal_is_new("poor", FEATHER, "another error"), "a new refusal: logged");
    cleanup(&path);
}

/// Review lot 4, point 7: a need bid gone between the brain's read and
/// the refresh (filled, its lots in delivery) is never re-placed by the
/// refresh: `None`, the caller's in-flight check decides.
#[test]
fn a_need_bid_gone_meanwhile_is_never_replaced_by_the_refresh() {
    let (market, path) = service("gone");
    funded_with(&market, "smith", 10_000);
    let StandingBid::Placed { order, .. } = keep_need_bid(&market, "smith", FEATHER, 10, 1, 5).expect("placed") else { panic!("placed") };
    let cancel = market.handle(&object(vec![("op", str_value("cancel")), ("account", str_value("smith")), ("order", Value::Int(order))]));
    assert_eq!(cancel.get("ok").as_bool(), Some(true));
    assert_eq!(refresh_need_bid(&market, "smith", FEATHER, 10, 1, 5).expect("no error"), None);
    assert!(resting(&market, "smith").is_empty(), "no bid placed by the refresh");
    cleanup(&path);
}

fn now_wall() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Re-review lot 4, point 1: a remainder cut is no fill. The next walk
/// window raises the need bid (never the backoff of a filled bid) and
/// records no own fill.
#[test]
fn a_remainder_cut_is_no_fill_for_the_walk() {
    let (market, path) = service("cutwalk");
    funded_with(&market, "miner", 10_000);
    keep_need_bid(&market, "miner", FEATHER, 10, 1, 100).expect("placed");
    let cut = refresh_need_bid(&market, "miner", FEATHER, 4, 10, 100).expect("refreshed").expect("resting");
    assert_eq!((cut.price, cut.remaining), (10, 4), "{cut:?}");
    // One need window (500 ticks at 600 ms) later.
    crate::market_reprice::reprice_round(&market, now_wall() + 500.0 * 0.6 + 1.0, 600.0);
    let row = resting(&market, "miner").pop().expect("still resting");
    assert!(row.price > 10, "the walk goes on up: {row:?}");
    let bounds = crate::market_reprice::bounds_of(&market, row.order_id).expect("walking");
    assert_eq!(bounds.own_fill, None, "no fill recorded");
    cleanup(&path);
}

/// Re-review lot 4, point 2: an amend that raises the price and cuts the
/// remainder is covered on the NEW remainder (10 at 5 locked 50 of 100
/// coins; 2 at 20 need 40).
#[test]
fn a_raise_with_a_cut_is_covered_on_the_new_remainder() {
    let (market, path) = service("cutcover");
    funded_with(&market, "buyer", 100);
    let placed = market.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(FEATHER)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    let order = placed.get("order").as_i64().expect("placed");
    let amended = market.handle(&object(vec![
        ("op", str_value("amend")),
        ("account", str_value("buyer")),
        ("order", Value::Int(order)),
        ("price", Value::Int(20)),
        ("max_remaining", Value::Int(2)),
    ]));
    assert_eq!(amended.get("ok").as_bool(), Some(true), "{amended:?}");
    assert_eq!(resting(&market, "buyer").iter().map(|row| (row.remaining, row.price)).collect::<Vec<_>>(), vec![(2, 20)]);
    cleanup(&path);
}
