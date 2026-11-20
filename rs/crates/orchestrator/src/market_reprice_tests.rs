//! Demand-driven repricing on the real service (temp database, no daemon).

use lifescape_json::{object, str_value, Value};

use super::{adopt, bounds_of, keep_bid, reprice_round, tracked, QuoteBounds, StandingBid};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("lifescape-reprice-{}-{tag}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
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

fn place(service: &MarketService, pairs: Vec<(&str, Value)>) -> Value {
    let mut all = vec![("op", str_value("place")), ("item", Value::Int(FEATHER))];
    all.extend(pairs);
    service.handle(&object(all))
}

fn price_of(service: &MarketService, account: &str) -> Option<i64> {
    service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .find(|row| matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0)
        .map(|row| row.price)
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-journal"));
}

fn now_wall() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Wall seconds of `ticks` at the 600 ms tick the tests pass.
fn ticks(count: f64) -> f64 {
    count * 0.6
}

/// Plan marché M3: an ask that sold backs off one ZIP step (the rest asks
/// higher). The 10 % walk (user 2026-11-19): idle, it drops 10 % per
/// window (at least a coin), under its own floor, down to one coin, and
/// is never withdrawn.
#[test]
fn filled_asks_back_off_idle_asks_walk_down_to_one() {
    let (service, path) = service("asks");
    funded(&service, "maker");
    funded(&service, "taker");
    let start = now_wall();
    let ask = place(&service, vec![("account", str_value("maker")), ("side", str_value("sell")), ("qty", Value::Int(10)), ("price", Value::Int(5)), ("floor", Value::Int(4)), ("patience", Value::Int(1))]);
    assert_eq!(ask.get("ok").as_bool(), Some(true), "{ask:?}");
    let take = place(&service, vec![("account", str_value("taker")), ("side", str_value("buy")), ("qty", Value::Int(2)), ("price", Value::Int(5))]);
    assert_eq!(take.get("status").as_str(), Some("filled"), "{take:?}");
    crate::market_test_support::settle_matched(&service);
    let restart = start + 10.0;
    assert_eq!(reprice_round(&service, restart, 600.0), 1);
    assert_eq!(price_of(&service, "maker"), Some(6), "sold at 5: the rest asks 6");
    let mut seen = Vec::new();
    for window in 1..=6 {
        reprice_round(&service, restart + 10.0 * f64::from(window), 600.0);
        seen.push(price_of(&service, "maker"));
    }
    assert_eq!(seen, [Some(5), Some(4), Some(3), Some(2), Some(1), Some(1)], "a coin a window under 10, through its floor 4, resting at 1");
    // No withdrawal any more: the sale alone is in the carnet.
    assert_eq!(service.lock_store().seller_learning("maker", 0).expect("learning").get(&FEATHER), Some(&(0, 1)));
    cleanup(&path);
}

/// A standing bid keeps its walk across passes; only its ceiling
/// follows the wallet, and a shrunk ceiling pulls the price down to it.
#[test]
fn standing_bids_keep_their_walk() {
    let (service, path) = service("standing");
    funded(&service, "fighter");
    let start = now_wall();
    let first = keep_bid(&service, "fighter", FEATHER, 1, 10, "gear").expect("placed");
    let StandingBid::Placed { order, price } = first else { panic!("{first:?}") };
    assert_eq!(price, 1, "untraded: opens at the 1-coin reference");
    assert_eq!(bounds_of(&service, order).map(|bounds| bounds.ceiling), Some(10));
    for window in 1..=4 {
        // One gear window: 2000 ticks.
        reprice_round(&service, start + ticks(2000.0 * f64::from(window)) + 1.0, 600.0);
    }
    // The 10 % walk (user 2026-11-19), at least a coin a window.
    assert_eq!(price_of(&service, "fighter"), Some(5), "four windows: 1 -> 5");
    let kept = keep_bid(&service, "fighter", FEATHER, 1, 30, "gear").expect("kept");
    assert!(matches!(kept, StandingBid::Kept { price: 5, .. }), "no reset to the reference: {kept:?}");
    let resting = service.lock_store().account_orders("fighter").unwrap_or_default().into_iter().find(|row| row.remaining > 0).expect("rests");
    assert_eq!(bounds_of(&service, resting.order_id).map(|bounds| bounds.ceiling), Some(30), "the ceiling rose with the wallet");
    let shrunk = keep_bid(&service, "fighter", FEATHER, 1, 2, "gear").expect("kept");
    assert!(matches!(shrunk, StandingBid::Kept { price: 2, .. }), "{shrunk:?}");
    assert_eq!(price_of(&service, "fighter"), Some(2));
    cleanup(&path);
}

/// A listing without bounds (placed before a restart) is adopted once
/// and walks from then on.
#[test]
fn adopted_listings_walk() {
    let (service, path) = service("adopt");
    funded(&service, "old_seller");
    let listed = place(&service, vec![("account", str_value("old_seller")), ("side", str_value("sell")), ("qty", Value::Int(50)), ("price", Value::Int(9))]);
    let order = listed.get("order").as_i64().expect("order");
    assert_eq!(tracked(&service), 0, "fixed price: no bounds");
    let row = service.lock_store().get_order(order).ok().flatten().expect("listed row");
    let bounds = QuoteBounds { patience_ticks: 1, last_change_wall: 0.0, listed_wall: 0.0, ..QuoteBounds::for_row(&row, 1, super::SELL_CAP) };
    assert!(adopt(&service, order, bounds.clone()));
    assert!(!adopt(&service, order, bounds.clone()), "adopted once");
    // Idempotent even when its bounds are dropped (2026-11-14: hundreds
    // of re-adoptions per bot per pass in the live log).
    service.lock_engine().quote_bounds.remove(&order);
    assert!(super::handled(&service, order));
    assert!(!adopt(&service, order, bounds.clone()), "never re-adopted");
    service.lock_engine().quote_bounds.insert(order, bounds.clone());
    reprice_round(&service, bounds.curve_start_wall + ticks(18_000.0), 600.0);
    assert_eq!(price_of(&service, "old_seller"), Some(8), "idle: walks down 10 %, at least a coin");
    cleanup(&path);
}

/// Daemon CPU (2026-11-14): a resting order inside its patience window
/// (default 2000 ticks) is not touched; an ask resting at one coin (the
/// end of its walk) never amends (its id stays, no new row).
#[test]
fn reprice_work_follows_change_not_order_count() {
    assert_eq!(super::PATIENCE_TICKS, 2000);
    assert_eq!(lifescape_market::zip::FAST_FILL_TICKS, super::PATIENCE_TICKS, "a fast fill is one inside the first window");
    let (service, path) = service("cadence");
    funded(&service, "parked");
    let listed = place(&service, vec![("account", str_value("parked")), ("side", str_value("sell")), ("qty", Value::Int(10)), ("price", Value::Int(1)), ("floor", Value::Int(1))]);
    let order = listed.get("order").as_i64().expect("order");
    let start = bounds_of(&service, order).map(|bounds| bounds.last_change_wall).expect("walks");
    assert_eq!(reprice_round(&service, start + 1000.0 * 0.6, 600.0), 0, "1000 ticks: inside the window");
    assert_eq!(reprice_round(&service, start + 2100.0 * 0.6, 600.0), 0, "at one coin: parked");
    let rows = service.lock_store().account_orders("parked").unwrap_or_default();
    assert_eq!(rows.len(), 1, "no amend, no replacement row: {rows:?}");
    assert_eq!(rows[0].order_id, order);
    cleanup(&path);
}

/// Plan marché M1/M2 (C1, C3): a need bid walked by amends keeps its
/// lineage (root, t0, motive, no true cancel), and the next obtain on the
/// object takes that bid over instead of cancelling it and re-placing it
/// lower.
#[test]
fn a_need_bid_keeps_its_lineage_across_amends_and_obtains() {
    let (service, path) = service("lineage");
    funded(&service, "fiona_park");
    let first = super::keep_bid_from(&service, "fiona_park", FEATHER, 5, 1, 10, crate::market_lineage::NEED).expect("placed");
    let StandingBid::Placed { order: root, .. } = first else { panic!("{first:?}") };
    let root_row = service.lock_store().get_order(root).ok().flatten().expect("root row");
    // A need: 500-tick windows, the 10 % walk (at least a coin).
    reprice_round(&service, now_wall() + ticks(500.0) + 1.0, 600.0);
    assert_eq!(price_of(&service, "fiona_park"), Some(2), "one window: 1 -> 2");
    let kept = super::keep_bid_from(&service, "fiona_park", FEATHER, 5, 1, 10, crate::market_lineage::NEED).expect("kept");
    let StandingBid::Kept { order, price: 2 } = kept else { panic!("taken over at its walked price: {kept:?}") };
    let row = service.lock_store().get_order(order).ok().flatten().expect("walked row");
    assert_eq!((row.root_id, row.root_clock, row.motive.as_str()), (root, root_row.created_clock, "need"));
    assert!(row.replaces.is_some(), "an amend links its predecessor");
    assert_eq!(bounds_of(&service, order).map(|bounds| (bounds.root_id, bounds.root_clock, bounds.motive)), Some((root, root_row.created_clock, "need".to_string())));
    let rows = service.lock_store().account_orders("fiona_park").unwrap_or_default();
    assert_eq!(rows.len(), 1, "one resting bid, never a second lineage: {rows:?}");
    let replaced = service.lock_store().get_order(root).ok().flatten().expect("root row");
    assert_eq!(replaced.cancel_reason, lifescape_store::AMENDED_REASON, "the walk is no cancellation");
    cleanup(&path);
}

/// Plan marché M1: after a daemon restart the engine's bounds are gone;
/// the kept bid's new bounds read its lineage and t0 from the carnet,
/// never restart them at the adoption.
#[test]
fn adoption_after_a_restart_reads_t0_from_the_carnet() {
    let (service, path) = service("restart");
    funded(&service, "ryan_mason");
    let first = super::keep_bid_from(&service, "ryan_mason", FEATHER, 1, 1, 30, crate::market_lineage::GEAR).expect("placed");
    let StandingBid::Placed { order: root, .. } = first else { panic!("{first:?}") };
    reprice_round(&service, 4_000_001_200.0, 600.0);
    let walked = service.lock_store().account_orders("ryan_mason").unwrap_or_default().into_iter().next().expect("walked");
    assert_ne!(walked.order_id, root, "amended once");
    drop(service);
    let catalogue = vec![
        CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    let reopened = MarketService::open(&path, catalogue).expect("reopens");
    assert_eq!(tracked(&reopened), 0, "bounds died with the daemon");
    funded(&reopened, "ryan_mason");
    let kept = super::keep_bid_from(&reopened, "ryan_mason", FEATHER, 1, 1, 30, crate::market_lineage::GEAR).expect("kept");
    let StandingBid::Kept { order, .. } = kept else { panic!("{kept:?}") };
    let bounds = bounds_of(&reopened, order).expect("adopted");
    assert_eq!((bounds.root_id, bounds.root_clock, bounds.motive.as_str()), (root, walked.root_clock, "gear"));
    assert_eq!(bounds.horizon_ticks, crate::market_lineage::horizon_ticks("gear", false));
    cleanup(&path);
}

/// Plan marché M7: a bid on an object nobody traded opens on its public
/// anchors; with no value known it opens at 1 coin and concedes upward
/// (no sellers' floor, user 2026-11-18: "retire les floor de prix").
#[test]
fn a_never_traded_bid_opens_on_the_public_anchors() {
    const BRONZE_BAR: i64 = 2349;
    let path = std::env::temp_dir().join(format!("lifescape-reprice-{}-discovery.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let catalogue = vec![
        CatalogueItem { id: BRONZE_BAR, name: "Bronze bar".to_string(), shop_price: 8, alch_value: 3 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    let service = MarketService::open(&path, catalogue).expect("opens");
    let reply = service.handle(&object(vec![("op", str_value("fund")), ("account", str_value("smith")), ("coins", Value::Int(1_000_000))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
    let placed = super::keep_bid_from(&service, "smith", BRONZE_BAR, 5, 1, 500, crate::market_lineage::NEED).expect("placed");
    assert!(matches!(placed, StandingBid::Placed { price: 1, .. }), "no floor: the lowest offer: {placed:?}");
    cleanup(&path);
}

/// User 2026-11-18 (levier 2) : lily_north bid 563 air runes @ 6 with a
/// ceiling of 20 and her own make cost at 31, then walked to 20 only
/// after 6000 ticks: the demand stayed invisible to the producers. A
/// need bid opens at what the object is worth to the buyer (its own make
/// cost, capped by the wallet), and a resting one under that worth is
/// raised to it at once (lineage kept).
#[test]
fn a_need_bid_opens_and_rises_to_the_buyers_worth() {
    let (market, path) = service("worth");
    funded(&market, "lily_north");
    let first = super::keep_bid_from(&market, "lily_north", FEATHER, 5, 1, 30, crate::market_lineage::NEED).expect("placed");
    let StandingBid::Placed { order: root, price } = first else { panic!("{first:?}") };
    assert!(price < 8, "no worth known: the old opening, got {price}");
    let raised = super::keep_bid_from(&market, "lily_north", FEATHER, 5, 8, 30, crate::market_lineage::NEED).expect("kept");
    let StandingBid::Kept { order, price: 8 } = raised else { panic!("raised to its worth: {raised:?}") };
    let row = market.lock_store().get_order(order).ok().flatten().expect("raised row");
    assert_eq!(row.root_id, root, "the lineage goes on");
    let capped = super::keep_bid_from(&market, "lily_north", FEATHER, 5, 50, 20, crate::market_lineage::NEED).expect("kept");
    let StandingBid::Kept { price: 20, .. } = capped else { panic!("never over the wallet: {capped:?}") };
    let (fresh, fresh_path) = service("worth-new");
    funded(&fresh, "ryan_park");
    let opened = super::keep_bid_from(&fresh, "ryan_park", FEATHER, 5, 12, 30, crate::market_lineage::NEED).expect("placed");
    let StandingBid::Placed { price: 12, .. } = opened else { panic!("a new need bid opens at its worth: {opened:?}") };
    cleanup(&path);
    cleanup(&fresh_path);
}

/// Plan prix §2 (user 2026-11-18): a standing bid that knew the object's
/// worth at or over its ceiling opened AT the ceiling (88 of 604 bids):
/// no course to concede. A gear bid opens under its ceiling by the
/// buyer's margin; a blocking need still opens at what it is worth.
#[test]
fn a_standing_bid_opens_under_its_ceiling_unless_needed() {
    let (service, path) = service("bid-room");
    funded(&service, "fighter");
    funded(&service, "smith");
    let margin = crate::market_discovery::account_margin(&service, "fighter");
    let gear = super::keep_bid_from(&service, "fighter", FEATHER, 1, 100, 100, "gear").expect("placed");
    let StandingBid::Placed { price, .. } = gear else { panic!("{gear:?}") };
    assert_eq!(price, lifescape_market::discovery::bid_under_ceiling(100, margin), "worth 100, ceiling 100: opens under it");
    let need = super::keep_bid_from(&service, "smith", FEATHER, 1, 100, 100, "need").expect("placed");
    let StandingBid::Placed { price, .. } = need else { panic!("{need:?}") };
    assert_eq!(price, 100, "a blocking need bids its worth");
    cleanup(&path);
}

/// Plan prix §6 (audit 2026-11-18: 8 142 amends refused in a day): an
/// order out of the book (matched, its deal in delivery) waits for the
/// deal; the pass never tries to amend it.
#[test]
fn an_order_in_delivery_is_not_repriced() {
    let (service, path) = service("in-flight");
    funded(&service, "maker");
    let listed = place(&service, vec![("account", str_value("maker")), ("side", str_value("sell")), ("qty", Value::Int(10)), ("price", Value::Int(9)), ("floor", Value::Int(1)), ("patience", Value::Int(1))]);
    let order = listed.get("order").as_i64().expect("order");
    let start = now_wall();
    service.lock_engine().books.get_mut(&FEATHER).expect("book").cancel(order as u64).expect("out of the book");
    assert_eq!(reprice_round(&service, start + ticks(18_000.0), 600.0), 0, "no amend tried");
    assert_eq!(price_of(&service, "maker"), Some(9));
    assert!(bounds_of(&service, order).is_some(), "still tracked for when its deal fails");
    cleanup(&path);
}

fn funded_with(service: &MarketService, account: &str, coins: i64) {
    let reply = service.handle(&object(vec![("op", str_value("fund")), ("account", str_value(account)), ("coins", Value::Int(coins))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

/// User 2026-11-19 (point 1): an account never promises more coins than
/// it holds. Live: hana_91 held 144 022 coins with 148 893 locked in 29
/// bids, the 10 % walk had raised them past its wallet (amends were never
/// checked). Every raise of a bid is covered by the free coins, or refused.
#[test]
fn a_bid_raise_needs_free_coins() {
    assert_eq!(crate::market_audit::affordable_raise(FEATHER, 10, 5, 10, 30), 8, "+1 a unit costs 10: three steps");
    assert_eq!(crate::market_audit::affordable_raise(FEATHER, 10, 5, 10, 0), 5, "no free coin: no raise");
    assert_eq!(crate::market_audit::affordable_raise(FEATHER, 10, 5, 10, 1000), 10, "covered: the whole raise");
    assert_eq!(crate::market_audit::affordable_raise(FEATHER, 10, 5, 3, 0), 3, "a cut always passes");
    let (service, path) = service("raise");
    funded_with(&service, "buyer", 100);
    let bid = place(&service, vec![("account", str_value("buyer")), ("side", str_value("buy")), ("qty", Value::Int(10)), ("price", Value::Int(5))]);
    let order = bid.get("order").as_i64().expect("placed");
    let refused = service.handle(&object(vec![("op", str_value("amend")), ("account", str_value("buyer")), ("order", Value::Int(order)), ("price", Value::Int(20))]));
    assert_eq!(refused.get("ok").as_bool(), Some(false), "+150 with 50 free: {refused:?}");
    assert_eq!(price_of(&service, "buyer"), Some(5), "the bid rests untouched");
    let raised = service.handle(&object(vec![("op", str_value("amend")), ("account", str_value("buyer")), ("order", Value::Int(order)), ("price", Value::Int(10))]));
    assert_eq!(raised.get("ok").as_bool(), Some(true), "+50 with 50 free: {raised:?}");
    cleanup(&path);
}

/// The 10 % walk stops where the free coins stop: 60 coins, a bid of
/// 10 feathers at 5 (50 locked) walks to 6 (+10) and holds there.
#[test]
fn the_bid_walk_stops_at_the_wallet() {
    let (service, path) = service("walkcap");
    funded_with(&service, "buyer", 60);
    let start = now_wall();
    let bid = place(&service, vec![("account", str_value("buyer")), ("side", str_value("buy")), ("qty", Value::Int(10)), ("price", Value::Int(5)), ("ceiling", Value::Int(100)), ("patience", Value::Int(1))]);
    assert_eq!(bid.get("ok").as_bool(), Some(true), "{bid:?}");
    for window in 1..=4 {
        reprice_round(&service, start + 10.0 * f64::from(window), 600.0);
    }
    assert_eq!(price_of(&service, "buyer"), Some(6), "never past its 60 coins");
    cleanup(&path);
}

/// Live 2026-11-20: bids at 1 coin frozen 18 to 67 h (kai_ellis 12 070
/// bronze 2h swords, tim_ford 9 719 raw lobsters), their walk lost at a
/// daemon restart. Every resting bid nobody walks is adopted with its
/// stored ceiling, else what the buyer can pay; the makers' quotes climb
/// in their own pass; asks and walked bids are left alone.
#[test]
fn every_bid_nobody_walks_is_adopted_with_its_ceiling() {
    use lifescape_store::{Order, OrderSide, OrderStatus};
    let row = |order_id: i64, side: OrderSide, motive: &str, ceiling: Option<i64>| Order { order_id, account: "t_adopt".to_string(), side, item_id: 314, quantity: 10, remaining: 10, price: 1, status: OrderStatus::Open, motive: motive.to_string(), ceiling, ..Order::default() };
    let rows = vec![
        row(1, OrderSide::Buy, "", None),
        row(2, OrderSide::Buy, "need", Some(80)),
        row(3, OrderSide::Buy, crate::market_lineage::MARKET_MAKE, None),
        row(4, OrderSide::Sell, "loot", None),
        row(5, OrderSide::Buy, "gear", Some(50)),
    ];
    let walked = |order: i64| order == 5;
    assert_eq!(super::bid_adoptions(&rows, &walked), vec![(1, super::UNSAID_CEILING), (2, 80)]);
    // A live motive without a stored ceiling waits for its owner's pass.
    let stock = vec![row(6, OrderSide::Buy, "stock", None)];
    assert!(super::bid_adoptions(&stock, &|_| false).is_empty());
}

/// Live 2026-11-20: eva_dale's orphan-walked nature rune bid reached
/// 122 298 coins, her free coins 0. An orphan bid engages at most
/// `UNSAID_SHARE_PCT` of the coins (free plus its own).
#[test]
fn an_orphan_bid_never_takes_the_wallet() {
    assert_eq!(super::unsaid_room(false, 5_000, 100), 5_000, "a stated ceiling: the free coins");
    assert_eq!(super::unsaid_room(true, 9_900, 100), 900, "10 % of 10 000, less its own 100");
    assert_eq!(super::unsaid_room(true, 0, 122_298), 0, "already over its share: no raise");
}

#[test]
fn a_maker_bid_bought_since_the_last_pass_was_served() {
    use lifescape_store::{Deal, DealStatus};
    let deal = |buyer: &str, item_id: i64, created_clock: i64| Deal {
        deal_id: 1,
        buyer: buyer.to_string(),
        seller: "seller".to_string(),
        item_id,
        quantity: 1,
        price: 5,
        meeting_x: 0,
        meeting_y: 0,
        meeting_level: 0,
        status: DealStatus::Settled,
        fail_reason: String::new(),
        deadline_tick: 0,
        created_clock,
        updated_clock: created_clock,
        taker_side: None,
    };
    let deals = vec![deal("maker", 314, 1_000), deal("other", 526, 2_000)];
    assert!(crate::market_maker::bought_since("maker", 314, &deals, 900.0));
    assert!(!crate::market_maker::bought_since("maker", 314, &deals, 1_100.0), "an older buy: unserved this pass");
    assert!(!crate::market_maker::bought_since("maker", 526, &deals, 900.0), "another's buy");
}
