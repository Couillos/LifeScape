#![forbid(unsafe_code)]
//! Order book integrity (review 2026-11-13): partly filled remainders
//! are cancellable, the fold never loses lots, the book and the carnet
//! never disagree after cancel/amend, a restart gives back the lots of
//! dead deals, and banknote orders lock and cover their base object.
//! Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_market::Side;
use lifescape_store::{OrderSide, OrderStatus};

use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const FEATHER: i64 = 314;
const RAW_SHRIMP: i64 = 317;

fn shrimp_note() -> i64 {
    lifescape_gamedata::objs_by_name("cert_raw_shrimp").into_iter().next().map(|obj| obj.obj_id as i64).expect("cert_raw_shrimp in the dump")
}

fn catalogue() -> Vec<CatalogueItem> {
    vec![
        CatalogueItem { id: FEATHER, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: RAW_SHRIMP, name: "Raw shrimps".to_string(), shop_price: 5, alch_value: 2 },
        CatalogueItem { id: shrimp_note(), name: "Raw shrimps".to_string(), shop_price: 5, alch_value: 2 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ]
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-market-integrity-{}-{tag}-{stamp}.db", std::process::id()))
}

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = temp_db(tag);
    (MarketService::open(&path, catalogue()).expect("opens"), path)
}

fn cleanup(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn funded(service: &MarketService, account: &str, item: i64) {
    let reply = service.handle(&object(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn place(service: &MarketService, account: &str, side: &str, item: i64, qty: i64, price: i64) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value(side)),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]))
}

fn status_of(service: &MarketService, order: i64) -> OrderStatus {
    service.lock_store().get_order(order).expect("reads").expect("row").status
}

fn book_sells(service: &MarketService, item: i64) -> Vec<(i64, i64)> {
    service.lock_engine().books.get(&item).map(|book| book.depth(Side::Sell, 20)).unwrap_or_default()
}

fn book_buys(service: &MarketService, item: i64) -> Vec<(i64, i64)> {
    service.lock_engine().books.get(&item).map(|book| book.depth(Side::Buy, 20)).unwrap_or_default()
}

/// Finding 1: a partly filled order (`matched`, remainder resting) is
/// withdrawn like an open one; before, the Short path left the
/// defaulter's phantom remainder matching until the next audit.
#[test]
fn partly_filled_remainder_is_cancelled() {
    let (service, path) = service("remainder");
    funded(&service, "seller", FEATHER);
    funded(&service, "buyer", FEATHER);
    let order = place(&service, "seller", "sell", FEATHER, 10, 4).get("order").as_i64().expect("order");
    assert_eq!(place(&service, "buyer", "buy", FEATHER, 4, 4).get("status").as_str(), Some("filled"));
    assert_eq!(status_of(&service, order), OrderStatus::Matched, "6 left resting");
    assert_eq!(service.cancel_open_for("seller", FEATHER, OrderSide::Sell), 1, "the remainder goes");
    assert_eq!(status_of(&service, order), OrderStatus::Cancelled);
    assert!(book_sells(&service, FEATHER).is_empty(), "nothing left on the book");
    cleanup(&path);
}

/// Finding 3 (cancel): the book is the authority. An order the book no
/// longer holds (a concurrent fill took it) is not "cancelled" in the
/// carnet: its fill writes the final status, a cancel never hides it.
#[test]
fn cancel_of_an_order_gone_from_the_book_leaves_the_carnet_to_the_fill() {
    let (service, path) = service("cancel-race");
    funded(&service, "seller", FEATHER);
    let order = place(&service, "seller", "sell", FEATHER, 5, 5).get("order").as_i64().expect("order");
    // Simulated race: a crossing place drained it, its row refresh is
    // still to come.
    service.lock_engine().books.get_mut(&FEATHER).expect("book").cancel(order as u64).expect("rests");
    let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value("seller")), ("order", Value::Int(order))]));
    assert_eq!(reply.get("ok").as_bool(), Some(false), "{reply:?}");
    assert_eq!(status_of(&service, order), OrderStatus::Open, "the fill's refresh owns the row");
    cleanup(&path);
}

/// Finding 3 (store): a final row (cancelled, settled) never comes back
/// through a late status write.
#[test]
fn final_rows_ignore_late_status_writes() {
    let (service, path) = service("final-guard");
    funded(&service, "seller", FEATHER);
    let order = place(&service, "seller", "sell", FEATHER, 5, 5).get("order").as_i64().expect("order");
    let reply = service.handle(&object(vec![("op", str_value("cancel")), ("account", str_value("seller")), ("order", Value::Int(order))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
    let late = service.lock_store().set_order_status(order, OrderStatus::Matched, 3, 1);
    assert!(late.is_err(), "late write refused: {late:?}");
    assert_eq!(status_of(&service, order), OrderStatus::Cancelled);
    cleanup(&path);
}

/// Finding 3 (amend): an amended partly filled order rests once, under
/// the replacement id, with the true remainder; the old row is closed.
#[test]
fn amend_of_a_partly_filled_order_rests_once() {
    let (service, path) = service("amend-partial");
    funded(&service, "seller", FEATHER);
    funded(&service, "buyer", FEATHER);
    let order = place(&service, "seller", "sell", FEATHER, 10, 4).get("order").as_i64().expect("order");
    place(&service, "buyer", "buy", FEATHER, 4, 4);
    let amended = service.handle(&object(vec![
        ("op", str_value("amend")),
        ("account", str_value("seller")),
        ("order", Value::Int(order)),
        ("price", Value::Int(6)),
    ]));
    assert_eq!(amended.get("ok").as_bool(), Some(true), "{amended:?}");
    let replacement = amended.get("order").as_i64().expect("replacement");
    assert_eq!(status_of(&service, order), OrderStatus::Cancelled);
    let row = service.lock_store().get_order(replacement).expect("reads").expect("row");
    assert_eq!((row.status, row.remaining, row.price), (OrderStatus::Open, 6, 6));
    assert_eq!(book_sells(&service, FEATHER), vec![(6, 6)]);
    // An order gone from the book cannot be amended, and no replacement
    // row is left behind.
    service.lock_engine().books.get_mut(&FEATHER).expect("book").cancel(replacement as u64).expect("rests");
    let refused = service.handle(&object(vec![
        ("op", str_value("amend")),
        ("account", str_value("seller")),
        ("order", Value::Int(replacement)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(refused.get("ok").as_bool(), Some(false));
    let live: Vec<i64> = service.lock_store().account_orders("seller").expect("reads").iter().map(|row| row.order_id).collect();
    assert_eq!(live, vec![replacement], "no phantom replacement");
    cleanup(&path);
}

/// Finding 2: the fold never loses lots. Fragments placed before the
/// first print sit above the new 5x band; merging them must not run
/// them through the placement guards again.
#[test]
fn fold_keeps_fragments_outside_the_price_band() {
    let (service, path) = service("fold-band");
    funded(&service, "fiona_park", FEATHER);
    funded(&service, "maker", FEATHER);
    funded(&service, "taker", FEATHER);
    for qty in [2, 13, 4] {
        assert_eq!(place(&service, "fiona_park", "sell", FEATHER, qty, 10).get("status").as_str(), Some("open"));
    }
    // A settled print at 1 coin: the band now caps new orders at 5.
    place(&service, "maker", "sell", FEATHER, 1, 1);
    place(&service, "taker", "buy", FEATHER, 1, 1);
    crate::market_test_support::settle_matched(&service);
    assert_eq!(place(&service, "fiona_park", "sell", FEATHER, 1, 10).get("ok").as_bool(), Some(false), "band on");
    // The ledger no longer covers anything either: the lots were
    // covered when placed.
    service.lock_engine().ledger.sync_account("fiona_park", 0, &[]);
    assert_eq!(service.consolidate_round(), 2, "three fragments become one");
    let live: Vec<(i64, i64)> =
        service.lock_store().account_orders("fiona_park").expect("reads").iter().map(|row| (row.price, row.remaining)).collect();
    assert_eq!(live, vec![(10, 19)], "one order of the summed lots");
    assert_eq!(book_sells(&service, FEATHER), vec![(10, 19)], "the book agrees");
    cleanup(&path);
}

/// The fold backs off when the book disagrees with the carnet (a fill
/// raced the round): nothing moves, no merged row is left behind.
#[test]
fn fold_backs_off_when_the_book_disagrees() {
    let (service, path) = service("fold-race");
    funded(&service, "fiona_park", FEATHER);
    let ids: Vec<i64> =
        [2, 13, 4].iter().map(|qty| place(&service, "fiona_park", "sell", FEATHER, *qty, 3).get("order").as_i64().expect("order")).collect();
    // Simulated race: the middle fragment left the book, its refresh is
    // still to come.
    service.lock_engine().books.get_mut(&FEATHER).expect("book").cancel(ids[1] as u64).expect("rests");
    assert_eq!(service.consolidate_round(), 0);
    let live: Vec<i64> = service.lock_store().account_orders("fiona_park").expect("reads").iter().map(|row| row.order_id).collect();
    assert_eq!(live, ids, "fragments untouched, no merged row");
    assert_eq!(book_sells(&service, FEATHER), vec![(3, 6)], "the two resting fragments are back");
    cleanup(&path);
}

/// Finding 4: a restart fails in-flight deals and gives their lots back
/// to both parties (neutral, like a runtime nav_down): the carnet rows
/// behind them were settled at match time.
#[test]
fn restart_relists_the_lots_of_dead_deals() {
    let (service, path) = service("restart-deals");
    funded(&service, "maker", FEATHER);
    funded(&service, "taker", FEATHER);
    let sell = place(&service, "maker", "sell", FEATHER, 10, 4).get("order").as_i64().expect("order");
    let bought = place(&service, "taker", "buy", FEATHER, 10, 4);
    assert_eq!(bought.get("status").as_str(), Some("filled"));
    assert_eq!(status_of(&service, sell), OrderStatus::Settled, "drained at match time");
    drop(service);
    let reopened = MarketService::open(&path, catalogue()).expect("reopens");
    let side_rows = |account: &str| -> Vec<(OrderSide, i64, i64)> {
        reopened.lock_store().account_orders(account).expect("reads").iter().map(|row| (row.side, row.remaining, row.price)).collect()
    };
    assert_eq!(side_rows("maker"), vec![(OrderSide::Sell, 10, 4)], "seller lots back");
    assert_eq!(side_rows("taker"), vec![(OrderSide::Buy, 10, 4)], "buyer lots back");
    assert_eq!(book_sells(&reopened, FEATHER), vec![(4, 10)]);
    assert_eq!(book_buys(&reopened, FEATHER), vec![(4, 10)]);
    assert!(reopened.lock_store().open_deals_for_account("maker").expect("reads").is_empty(), "deal failed");
    drop(reopened);
    // A second restart relists nothing more.
    let again = MarketService::open(&path, catalogue()).expect("reopens");
    assert_eq!(again.lock_store().account_orders("maker").expect("reads").len(), 1);
    cleanup(&path);
}

/// Finding 5: a sell placed on the banknote locks and is covered by the
/// base object (the bank holds base objects; the audit folds notes).
#[test]
fn note_orders_lock_and_cover_the_base_object() {
    let (service, path) = service("note-locks");
    let note = shrimp_note();
    assert_ne!(note, RAW_SHRIMP);
    assert_eq!(crate::market_audit::base_of(note), RAW_SHRIMP);
    funded(&service, "fisher", note);
    assert_eq!(place(&service, "fisher", "sell", note, 12, 2).get("status").as_str(), Some("open"));
    let locks = service.locks_for(&["fisher".to_string()]);
    let (_, items) = &locks["fisher"];
    assert_eq!(items.get(&RAW_SHRIMP), Some(&12), "the base stack is locked: {items:?}");
    assert_eq!(items.get(&note), None, "never the note id");
    let orders = service.lock_store().account_orders("fisher").expect("reads");
    let stock = std::collections::BTreeMap::from([(RAW_SHRIMP, 12i64)]);
    let bad = crate::market_audit::uncovered(&orders, 0, &stock, 0, &std::collections::BTreeMap::new());
    assert!(bad.is_empty(), "12 banked shrimps cover the note sell: {bad:?}");
    let promised = std::collections::BTreeMap::from([(note, 5i64)]);
    let short = crate::market_audit::uncovered(&orders, 0, &stock, 0, &promised);
    assert_eq!(short, vec![orders[0].order_id], "a note deal takes base stock too");
    cleanup(&path);
}
