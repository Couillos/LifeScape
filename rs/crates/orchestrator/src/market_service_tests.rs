#![forbid(unsafe_code)]
//! Fake-client tests: every op through `handle()`, no daemon, no game.
//! Temp database file per test, never `run/`.

use lifescape_json::{object, str_value, Value};
use lifescape_store::{NewExecution, YieldCandleResolution};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

fn catalogue() -> Vec<CatalogueItem> {
    vec![
        CatalogueItem { id: 314, name: "Feather".to_string(), shop_price: 2, alch_value: 1 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ]
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lifescape-market-svc-{}-{tag}-{stamp}.db", std::process::id()))
}

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    let path = temp_db(tag);
    let service = MarketService::open(&path, catalogue()).expect("opens");
    (service, path)
}

fn request(pairs: Vec<(&str, Value)>) -> Value {
    object(pairs)
}

fn funded(service: &MarketService, account: &str) {
    let reply = service.handle(&request(vec![
        ("op", str_value("fund")),
        ("account", str_value(account)),
        ("coins", Value::Int(1_000_000)),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10_000)),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
}

fn cleanup(path: &std::path::PathBuf) {
    let _ = std::fs::remove_file(path);
    let journal = path.with_extension("db-journal");
    let _ = std::fs::remove_file(journal);
}

#[test]
fn price_time_priority_and_partial_fills() {
    let (service, path) = service("priority");
    funded(&service, "seller-a");
    funded(&service, "seller-b");
    funded(&service, "buyer");
    let sell = |account: &str, price: i64| {
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(price)),
        ]))
    };
    assert_eq!(sell("seller-a", 5).get("status").as_str(), Some("open"));
    assert_eq!(sell("seller-b", 5).get("status").as_str(), Some("open"));
    // Buyer takes 15: oldest ask first, remainder rests on seller-b.
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(15)),
        ("price", Value::Int(6)),
    ]));
    assert_eq!(bought.get("ok").as_bool(), Some(true));
    assert_eq!(bought.get("fills").as_arr().map(|fills| fills.len()), Some(2));
    assert_eq!(bought.get("deals").as_arr().map(|deals| deals.len()), Some(2));
    // Seller-a filled, seller-b partially: one open order left.
    let orders = service.handle(&request(vec![("op", str_value("my_orders")), ("account", str_value("seller-b"))]));
    assert_eq!(orders.get("orders").as_arr().map(|rows| rows.len()), Some(1));
    assert_eq!(orders.get("orders").as_arr().unwrap()[0].get("left").as_i64(), Some(5));
    cleanup(&path);
}

#[test]
fn amend_is_cancel_and_replace_behind_the_queue() {
    let (service, path) = service("amend");
    funded(&service, "a");
    funded(&service, "b");
    funded(&service, "buyer");
    let place = |account: &str| {
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(5)),
            ("price", Value::Int(5)),
        ]))
    };
    let first = place("a").get("order").as_i64().unwrap();
    place("b");
    let amended = service.handle(&request(vec![
        ("op", str_value("amend")),
        ("account", str_value("a")),
        ("order", Value::Int(first)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(amended.get("ok").as_bool(), Some(true));
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(5)),
        ("price", Value::Int(5)),
    ]));
    let fills = bought.get("fills").as_arr().cloned().unwrap_or_default();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].get("seller").as_str(), Some("b"), "repriced order lost its queue");
    cleanup(&path);
}

#[test]
fn guards_reject_bands_and_naked_orders() {
    let (service, path) = service("guards");
    funded(&service, "poor");
    // Never traded (PRD3 lot 2): the reference is 1 coin and does not
    // cap anything, so a buyer may bid an untraded item up freely.
    let free_bid = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("poor")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(11)),
    ]));
    assert_eq!(free_bid.get("ok").as_bool(), Some(true), "untraded items have no band: {free_bid:?}");
    // Naked sell: no stock funded on this account.
    let naked = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("ghost")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(5)),
        ("price", Value::Int(2)),
    ]));
    assert_eq!(naked.get("ok").as_bool(), Some(false));
    // Naked buy: no coins.
    let broke = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("ghost")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(100)),
        ("price", Value::Int(2)),
    ]));
    assert_eq!(broke.get("ok").as_bool(), Some(false));
    cleanup(&path);
}

#[test]
fn cancel_closes_and_deal_walks_every_step() {
    let (service, path) = service("deal");
    funded(&service, "maker");
    funded(&service, "taker");
    let placed = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    let order = placed.get("order").as_i64().unwrap();
    let cancelled = service.handle(&request(vec![
        ("op", str_value("cancel")),
        ("account", str_value("maker")),
        ("order", Value::Int(order)),
    ]));
    assert_eq!(cancelled.get("ok").as_bool(), Some(true));
    assert_eq!(
        service
            .handle(&request(vec![
                ("op", str_value("cancel")),
                ("account", str_value("maker")),
                ("order", Value::Int(order)),
            ]))
            .get("ok")
            .as_bool(),
        Some(false),
        "double cancel is loud"
    );
    // Fresh match, then the whole delivery machine.
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("taker")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    let deal = bought.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().unwrap();
    // Fresh matches wait for a convocation before anyone may arrive.
    let convened = service.handle(&request(vec![
        ("op", str_value("convene")),
        ("deals", Value::Arr(vec![convene_payload(deal, 1000, 9000)])),
    ]));
    assert_eq!(convened.get("poses").as_arr().map(|rows| rows.len()), Some(2), "both parties posed");
    let arrived = |account: &str| {
        service.handle(&request(vec![
            ("op", str_value("deal_arrived")),
            ("account", str_value(account)),
            ("deal", Value::Int(deal)),
        ]))
    };
    assert_eq!(arrived("maker").get("status").as_str(), Some("convened"));
    assert_eq!(arrived("taker").get("status").as_str(), Some("both_arrived"));
    let report = |outcome: &str| {
        service.handle(&request(vec![
            ("op", str_value("deal_report")),
            ("account", str_value("maker")),
            ("deal", Value::Int(deal)),
            ("outcome", str_value(outcome)),
        ]))
    };
    assert_eq!(report("trading").get("status").as_str(), Some("trading"));
    assert_eq!(report("settled").get("status").as_str(), Some("settled"));
    assert_eq!(report("settled").get("ok").as_bool(), Some(false), "settled is terminal");
    cleanup(&path);
}

#[test]
fn every_open_deal_state_fails_with_its_reason() {
    for reason in ["no_show", "mismatch", "death", "timeout", "nav_down"] {
        let (service, path) = service(reason);
        funded(&service, "maker");
        funded(&service, "taker");
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value("maker")),
            ("side", str_value("sell")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(4)),
        ]));
        let bought = service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value("taker")),
            ("side", str_value("buy")),
            ("item", Value::Int(314)),
            ("qty", Value::Int(10)),
            ("price", Value::Int(5)),
        ]));
        let deal = bought.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().unwrap();
        let failed = service.handle(&request(vec![
            ("op", str_value("deal_report")),
            ("account", str_value("taker")),
            ("deal", Value::Int(deal)),
            ("outcome", str_value("failed")),
            ("reason", str_value(reason)),
        ]));
        assert_eq!(failed.get("status").as_str(), Some("failed"), "reason {reason}");
        cleanup(&path);
    }
}

#[test]
fn quote_history_and_tick_serve_the_front() {
    let (service, path) = service("front");
    funded(&service, "maker");
    funded(&service, "taker");
    let quote = service.handle(&request(vec![("op", str_value("quote")), ("item", Value::Int(314))]));
    assert_eq!(quote.get("ok").as_bool(), Some(true));
    assert_eq!(quote.get("reference").as_i64(), Some(1), "never traded: 1 coin, the shop never anchors");
    assert_eq!(quote.get("traded").as_bool(), Some(false));
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("maker")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("taker")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    crate::market_test_support::settle_matched(&service);
    let history = service.handle(&request(vec![
        ("op", str_value("history")),
        ("item", Value::Int(314)),
        ("res", str_value("1h")),
    ]));
    assert_eq!(history.get("ok").as_bool(), Some(true));
    assert_eq!(history.get("trades").as_arr().map(|rows| rows.len()), Some(1));
    let tick = service.handle(&request(vec![("op", str_value("tick")), ("since", Value::Int(0))]));
    assert_eq!(tick.get("trades").as_arr().map(|rows| rows.len()), Some(1));
    let book = service.handle(&request(vec![("op", str_value("book")), ("item", Value::Int(314))]));
    assert_eq!(book.get("ok").as_bool(), Some(true));
    cleanup(&path);
}

fn match_one(service: &MarketService, maker: &str, taker: &str, qty: i64, price: i64) -> i64 {
    service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value(maker)),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price)),
    ]));
    let bought = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value(taker)),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(price + 1)),
    ]));
    assert_eq!(bought.get("ok").as_bool(), Some(true));
    bought.get("deals").as_arr().cloned().unwrap_or_default()[0].as_i64().unwrap()
}

fn convene_payload(deal: i64, tick: i64, deadline: i64) -> Value {
    object(vec![
        ("deal", Value::Int(deal)),
        ("depart_buyer", Value::Int(tick + 30)),
        ("depart_seller", Value::Int(tick + 30)),
        ("deadline_buyer", Value::Int(deadline)),
        ("deadline_seller", Value::Int(deadline)),
        ("sweep_wall", Value::Float(deadline as f64)),
    ])
}

/// Convene hands adjacent tiles and regroups one bot's deals in a single
/// pose; a no-show relists the present party with its priority while the
/// absent loses everything.
#[test]
fn convene_tiles_and_no_show_relists_present() {
    let (service, path) = service("convene");
    funded(&service, "seller-a");
    funded(&service, "seller-b");
    funded(&service, "buyer");
    let first = match_one(&service, "seller-a", "buyer", 10, 4);
    let second = match_one(&service, "seller-b", "buyer", 6, 4);
    let pending = service.handle(&request(vec![("op", str_value("pending")), ("now_wall", Value::Float(1000.0)), ("tick_ms", Value::Int(60))]));
    assert_eq!(pending.get("deals").as_arr().map(|rows| rows.len()), Some(2));
    let reply = service.handle(&request(vec![
        ("op", str_value("convene")),
        ("deals", Value::Arr(vec![convene_payload(first, 1000, 9000), convene_payload(second, 1000, 9000)])),
    ]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
    let poses = reply.get("poses").as_arr().cloned().unwrap_or_default();
    assert_eq!(poses.len(), 3, "one pose per bot");
    let buyer_pose = poses.iter().find(|pose| pose.get("account").as_str() == Some("buyer")).expect("buyer posed");
    // Regrouped: both deals in one convocation, one tile each.
    assert_eq!(buyer_pose.get("deals").as_arr().map(|rows| rows.len()), Some(2));
    let tiles: Vec<String> = buyer_pose.get("deals").as_arr().cloned().unwrap_or_default().iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    assert!(tiles[0] != tiles[1], "distinct tiles {}", tiles.join(" "));
    let seller_tile = poses
        .iter()
        .find(|pose| pose.get("account").as_str() == Some("seller-a"))
        .expect("seller posed")
        .get("deals")
        .as_arr()
        .cloned()
        .unwrap_or_default()[0]
        .as_str()
        .unwrap_or("")
        .to_string();
    let buyer_first = tiles.iter().find(|tile| tile.starts_with(&format!("{first}="))).expect("buyer first tile");
    let (bx, by) = buyer_first.split_once('=').unwrap().1.split_once(',').unwrap();
    let (sx, sy) = seller_tile.split_once('=').unwrap().1.split_once(',').unwrap();
    let (bx, by, sx, sy): (i64, i64, i64, i64) = (bx.parse().unwrap(), by.parse().unwrap(), sx.parse().unwrap(), sy.parse().unwrap());
    assert!((bx - sx).abs() <= 1 && (by - sy).abs() <= 1, "pair {buyer_first} ~ {seller_tile} adjacent");
    // Only the buyer shows up: no-show fails the deal, the buyer relists
    // with priority, the seller loses theirs.
    service.handle(&request(vec![
        ("op", str_value("deal_arrived")),
        ("account", str_value("buyer")),
        ("deal", Value::Int(first)),
    ]));
    let swept = service.handle(&request(vec![
        ("op", str_value("sweep")),
        ("now_wall", Value::Float(9001.0)),
        ("nav_up", Value::Bool(true)),
    ]));
    assert_eq!(swept.get("failed").as_arr().map(|rows| rows.len()), Some(2), "both expire past deadline");
    let buyer_orders = service.handle(&request(vec![("op", str_value("my_orders")), ("account", str_value("buyer"))]));
    let buyer_rows = buyer_orders.get("orders").as_arr().cloned().unwrap_or_default();
    let open: Vec<&Value> =
        buyer_rows.iter().filter(|row| row.get("status").as_str() == Some("open")).collect();
    assert!(!open.is_empty(), "present party back on the carnet");
    assert!(open.iter().all(|row| row.get("price").as_i64() == Some(5)), "same price kept");
    let seller_orders = service.handle(&request(vec![("op", str_value("my_orders")), ("account", str_value("seller-a"))]));
    let seller_rows = seller_orders.get("orders").as_arr().cloned().unwrap_or_default();
    let seller_open = seller_rows.iter().filter(|row| row.get("status").as_str() == Some("open")).count();
    assert_eq!(seller_open, 0, "absent party re-places by hand");
    cleanup(&path);
}

/// Without nav every expiry is `nav_down`: neutral, both relist, nobody
/// travels blind. Ten simultaneous deals convene on distinct tiles.
#[test]
fn nav_down_fails_neutral_and_ten_deals_convene() {
    let (service, path) = service("navdown");
    // Full live-shaped ring (no snapshot in unit tests): 56 tiles.
    service.lock_engine().ring = crate::market_convene::build_ring(&|_, _| true);
    let mut deals = Vec::new();
    for index in 0..10 {
        let maker = format!("maker-{index}");
        let taker = format!("taker-{index}");
        funded(&service, &maker);
        funded(&service, &taker);
        deals.push(match_one(&service, &maker, &taker, 5, 4));
    }
    let payloads: Vec<Value> = deals.iter().map(|deal| convene_payload(*deal, 2000, 8000)).collect();
    let reply = service.handle(&request(vec![("op", str_value("convene")), ("now", Value::Int(2000)), ("deals", Value::Arr(payloads))]));
    assert_eq!(reply.get("poses").as_arr().map(|rows| rows.len()), Some(20), "twenty bots posed");
    let mut tiles = Vec::new();
    for pose in reply.get("poses").as_arr().cloned().unwrap_or_default() {
        for deal in pose.get("deals").as_arr().cloned().unwrap_or_default() {
            tiles.push(deal.as_str().unwrap_or("").to_string());
        }
    }
    tiles.sort();
    tiles.dedup();
    assert_eq!(tiles.len(), 20, "no double-booked tile");
    let swept = service.handle(&request(vec![
        ("op", str_value("sweep")),
        ("now_wall", Value::Float(8001.0)),
        ("nav_up", Value::Bool(false)),
    ]));
    assert_eq!(swept.get("failed").as_arr().map(|rows| rows.len()), Some(10), "all expire nav_down");
    // Both sides relist neutrally and re-enter as takers: each lot
    // crosses another failed pair's lot at once (never its own partner,
    // kept apart by the cooldown), so nothing rests crossed (2026-11-13).
    let rematched: Vec<(String, String)> = service
        .lock_engine()
        .deals
        .values()
        .filter(|deal| deal.status == lifescape_market::DealStatus::Matched)
        .map(|deal| (deal.buyer.clone(), deal.seller.clone()))
        .collect();
    assert_eq!(rematched.len(), 10, "every relisted lot matched again: {rematched:?}");
    for (buyer, seller) in &rematched {
        assert_ne!(buyer.trim_start_matches("taker-"), seller.trim_start_matches("maker-"), "failed pair never rematched");
    }
    let book = service.lock_engine().books.get(&314).map(|book| book.best()).unwrap_or((None, None));
    assert_eq!(book, (None, None), "no lot left resting, nothing crossed");
    cleanup(&path);
}

/// Fifty deals settle with zero ledger loss: every coin and feather ends
/// where the carnet said.
#[test]
fn fifty_deals_settle_without_loss() {
    let (service, path) = service("fifty");
    for index in 0..50 {
        let maker = format!("maker-{index}");
        let taker = format!("taker-{index}");
        funded(&service, &maker);
        funded(&service, &taker);
        let deal = match_one(&service, &maker, &taker, 10, 4);
        service.handle(&request(vec![
            ("op", str_value("convene")),
            ("deals", Value::Arr(vec![convene_payload(deal, 3000, 9000)])),
        ]));
        let arrived = |account: &str| {
            service.handle(&request(vec![
                ("op", str_value("deal_arrived")),
                ("account", str_value(account)),
                ("deal", Value::Int(deal)),
            ]))
        };
        arrived(&maker);
        arrived(&taker);
        for outcome in ["trading", "settled"] {
            let report = service.handle(&request(vec![
                ("op", str_value("deal_report")),
                ("account", str_value(&maker)),
                ("deal", Value::Int(deal)),
                ("outcome", str_value(outcome)),
            ]));
            assert_eq!(report.get("ok").as_bool(), Some(true), "deal {deal} {outcome}");
        }
    }
    let engine = service.lock_engine();
    let coins: i64 = (0..50).map(|index| engine.ledger.coins_of(&format!("maker-{index}")) + engine.ledger.coins_of(&format!("taker-{index}"))).sum();
    let stock: i64 =
        (0..50).map(|index| engine.ledger.stock_of(&format!("maker-{index}"), 314) + engine.ledger.stock_of(&format!("taker-{index}"), 314)).sum();
    // Funded 1M coins + 10k feathers per account, 100 accounts; every deal
    // moves 10 @ 5, settled fifty times.
    assert_eq!(coins, 100 * 1_000_000, "coins conserved");
    assert_eq!(stock, 100 * 10_000, "feathers conserved");
    cleanup(&path);
}

/// A daemon restart reloads the carnet: resting orders rest again under
/// their store ids, and the next crossing order matches them. Regression
/// test for the live L11 finding (engine empty, ~500 orphaned seed
/// orders, quotes without depth after a restart).
#[test]
fn restart_reloads_resting_orders_into_books() {
    let (service, path) = service("reload");
    funded(&service, "seller");
    funded(&service, "buyer");
    let sell = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("seller")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(sell.get("ok").as_bool(), Some(true));
    let buy = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(4)),
    ]));
    assert_eq!(buy.get("ok").as_bool(), Some(true), "resting bid, no cross");
    drop(service);
    // Same database file, new engine: the restart path.
    let reopened = MarketService::open(&path, catalogue()).expect("reopens");
    let book = reopened.handle(&request(vec![("op", str_value("book")), ("item", Value::Int(314))]));
    assert_eq!(book.get("ok").as_bool(), Some(true));
    assert_eq!(book.get("sells").as_arr().map(|rows| rows.len()), Some(1), "reloaded ask visible");
    assert_eq!(book.get("buys").as_arr().map(|rows| rows.len()), Some(1), "reloaded bid visible");
    funded(&reopened, "taker");
    let cross = reopened.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("taker")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(10)),
        ("price", Value::Int(6)),
    ]));
    assert_eq!(cross.get("ok").as_bool(), Some(true));
    assert_eq!(cross.get("fills").as_arr().map(|fills| fills.len()), Some(1), "cross matches the reloaded ask");
    cleanup(&path);
}


#[test]
fn list_merges_same_name_forms_and_sorts_by_volume() {
    let path = temp_db("list-merge");
    let catalogue = vec![
        CatalogueItem { id: 100, name: "Logs".to_string(), shop_price: 4, alch_value: 2 },
        CatalogueItem { id: 101, name: "Logs".to_string(), shop_price: 4, alch_value: 2 },
        CatalogueItem { id: 102, name: "Coal".to_string(), shop_price: 3, alch_value: 1 },
    ];
    let service = MarketService::open(&path, catalogue).expect("opens");
    let clock = MarketService::now_clock();
    let print = |item: i64, qty: i64, price: i64, at: i64| {
        service
            .lock_store()
            .record_execution(&lifescape_store::NewExecution {
                item_id: item,
                quantity: qty,
                price,
                clock: at,
                server_tick: at,
            })
            .expect("prints");
    };
    print(100, 5, 10, clock - 100);
    print(101, 7, 12, clock - 50);
    print(102, 3, 3, clock - 10);
    let list = service.handle(&request(vec![("op", str_value("list")), ("limit", Value::Int(2000))]));
    assert_eq!(list.get("ok").as_bool(), Some(true));
    let rows = list.get("items").as_arr().expect("item rows");
    assert_eq!(rows.len(), 2, "one row per displayed name, got {rows:?}");
    // Volume descends: Logs (5 + 7) before Coal (3).
    assert_eq!(rows[0].get("name").as_str(), Some("Logs"));
    assert_eq!(rows[0].get("volume24h").as_i64(), Some(12));
    // Canonical id carries the volume; VWAP prices the merged fills.
    assert_eq!(rows[0].get("item").as_i64(), Some(101));
    assert_eq!(rows[0].get("price").as_i64(), Some(11));
    // Change of the displayed VWAP (11) against the base (no previous
    // day: today's first print, 10): +10 %, not first-vs-last print.
    assert_eq!(rows[0].get("change24h").as_i64(), Some(100));
    assert_eq!(rows[1].get("name").as_str(), Some("Coal"));
    cleanup(&path);
}

fn print_at(service: &MarketService, clock: i64, price: i64, qty: i64) {
    service
        .lock_store()
        .record_execution(&NewExecution {
            item_id: 314,
            quantity: qty,
            price,
            clock,
            server_tick: clock,
        })
        .expect("prints");
}

/// History candles carry the execution count of their window (`n`,
/// tooltip §6.2): three fills in one hour read `n = 3`, one in the
/// next reads `n = 1`, beside the OHLCV the store rebuilt.
#[test]
fn history_candles_carry_execution_counts() {
    let (service, path) = service("candle-n");
    let hour = 48_000 * 3600;
    print_at(&service, hour + 60, 4, 2);
    print_at(&service, hour + 120, 6, 3);
    print_at(&service, hour + 300, 5, 4);
    print_at(&service, hour + 3600 + 10, 7, 10);
    service.rebuild_candles(314, YieldCandleResolution::Hour1).expect("rebuilds");
    let history = service.handle(&request(vec![
        ("op", str_value("history")),
        ("item", Value::Int(314)),
        ("res", str_value("1h")),
        ("from", Value::Int(hour - 3600)),
        ("to", Value::Int(hour + 3 * 3600)),
    ]));
    assert_eq!(history.get("ok").as_bool(), Some(true));
    assert_eq!(history.get("res").as_str(), Some("1h"));
    let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
    assert_eq!(candles.len(), 2);
    assert_eq!(candles[0].get("t").as_i64(), Some(hour));
    assert_eq!(candles[0].get("o").as_i64(), Some(4));
    assert_eq!(candles[0].get("h").as_i64(), Some(6));
    assert_eq!(candles[0].get("l").as_i64(), Some(4));
    assert_eq!(candles[0].get("c").as_i64(), Some(5));
    assert_eq!(candles[0].get("v").as_i64(), Some(9));
    assert_eq!(candles[0].get("n").as_i64(), Some(3), "three fills in the window");
    assert_eq!(candles[1].get("t").as_i64(), Some(hour + 3600));
    assert_eq!(candles[1].get("v").as_i64(), Some(10));
    assert_eq!(candles[1].get("n").as_i64(), Some(1), "single fill in the window");
    cleanup(&path);
}

/// History stays sparse: windows without executions return no candle,
/// so the front draws the fixed-slot gap (never a fake flat line).
#[test]
fn history_stays_sparse_for_empty_windows() {
    let (service, path) = service("candle-sparse");
    let hour = 48_000 * 3600;
    print_at(&service, hour + 60, 4, 2);
    print_at(&service, hour + 2 * 3600 + 60, 9, 5);
    service.rebuild_candles(314, YieldCandleResolution::Hour1).expect("rebuilds");
    let history = service.handle(&request(vec![
        ("op", str_value("history")),
        ("item", Value::Int(314)),
        ("res", str_value("1h")),
        ("from", Value::Int(hour - 3600)),
        ("to", Value::Int(hour + 4 * 3600)),
    ]));
    let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
    assert_eq!(candles.len(), 2, "the empty middle hour is the front's gap");
    assert_eq!(candles[0].get("t").as_i64(), Some(hour));
    assert_eq!(candles[1].get("t").as_i64(), Some(hour + 2 * 3600));
    assert_eq!(candles[1].get("n").as_i64(), Some(1));
    cleanup(&path);
}

/// A live fill repaints the history candles at once: `history` reads
/// the `candles` cache, never executions, so without a rebuild on the
/// fill path every live trade stays invisible on the market page.
#[test]
fn live_fill_repaints_history_candles() {
    let (service, path) = service("fill-candles");
    funded(&service, "seller-a");
    funded(&service, "buyer");
    match_one(&service, "seller-a", "buyer", 10, 4);
    // A match alone paints nothing (a promise, not a trade)...
    let before = service.handle(&request(vec![("op", str_value("history")), ("item", Value::Int(314)), ("res", str_value("1h"))]));
    assert!(before.get("candles").as_arr().map_or(true, |rows| rows.is_empty()), "no candle before the handover");
    // ...the settled handover repaints both resolutions, no manual rebuild.
    assert_eq!(crate::market_test_support::settle_matched(&service), 1);
    for res in ["1h", "1d"] {
        let history = service.handle(&request(vec![
            ("op", str_value("history")),
            ("item", Value::Int(314)),
            ("res", str_value(res)),
        ]));
        let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
        assert_eq!(candles.len(), 1, "{res} shows the live fill, got {candles:?}");
        assert_eq!(candles[0].get("v").as_i64(), Some(10));
        assert_eq!(candles[0].get("n").as_i64(), Some(1));
    }
    cleanup(&path);
}

/// Restart backfill: executions persisted but the `candles` cache empty
/// (old items whose fills predate the live repaint) still serve
/// history — `history` rebuilds from the store on demand, no live fill.
#[test]
fn restart_backfills_history_from_executions() {
    let (service, path) = service("candle-backfill");
    let hour = 48_000 * 3600;
    print_at(&service, hour + 60, 4, 2);
    print_at(&service, hour + 120, 6, 3);
    // No rebuild, no fill: the candle cache is empty like after a
    // restart for items whose fills predate the repaint path.
    for res in ["1h", "1d"] {
        let history = service.handle(&request(vec![
            ("op", str_value("history")),
            ("item", Value::Int(314)),
            ("res", str_value(res)),
            ("from", Value::Int(hour - 3600)),
            ("to", Value::Int(hour + 3 * 3600)),
        ]));
        let candles = history.get("candles").as_arr().cloned().unwrap_or_default();
        assert_eq!(candles.len(), 1, "{res} backfills from executions, got {candles:?}");
        assert_eq!(candles[0].get("v").as_i64(), Some(5));
        assert_eq!(candles[0].get("n").as_i64(), Some(2));
    }
    cleanup(&path);
}

#[test]
fn recent_lists_newest_deal_first_with_pseudos() {
    let (service, path) = service("recent");
    funded(&service, "seller-a");
    funded(&service, "buyer");
    let deal = match_one(&service, "seller-a", "buyer", 10, 4);
    let reply = service.handle(&request(vec![("op", str_value("recent")), ("limit", Value::Int(20))]));
    assert_eq!(reply.get("ok").as_bool(), Some(true));
    let rows = reply.get("deals").as_arr().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("deal").as_i64(), Some(deal));
    assert_eq!(rows[0].get("buyer").as_str(), Some("buyer"));
    assert_eq!(rows[0].get("seller").as_str(), Some("seller-a"));
    assert_eq!(rows[0].get("item").as_i64(), Some(314));
    assert_eq!(rows[0].get("qty").as_i64(), Some(10));
    assert_eq!(rows[0].get("price").as_i64(), Some(4));
    // Narrowing to an untraded item hides the deal, like `trades`.
    let narrowed = service.handle(&request(vec![
        ("op", str_value("recent")),
        ("item", Value::Int(995)),
        ("limit", Value::Int(20)),
    ]));
    assert_eq!(narrowed.get("deals").as_arr().map(|rows| rows.len()), Some(0));
    cleanup(&path);
}

#[test]
fn list_traded_only_hides_untraded_goods() {
    let (service, path) = service("traded-only");
    funded(&service, "seller-a");
    funded(&service, "buyer");
    let empty = service.handle(&request(vec![("op", str_value("list")), ("traded_only", Value::Int(1))]));
    assert_eq!(empty.get("items").as_arr().map(|rows| rows.len()), Some(0));
    match_one(&service, "seller-a", "buyer", 10, 4);
    crate::market_test_support::settle_matched(&service);
    let reply = service.handle(&request(vec![("op", str_value("list")), ("traded_only", Value::Int(1))]));
    let rows = reply.get("items").as_arr().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").as_str(), Some("Feather"));
    cleanup(&path);
}

/// PRD3 lot 2: the first print turns the band on. Before it, any bid
/// lists; after a trade at 2, a bid above 5x (10) is refused.
#[test]
fn first_print_turns_the_band_on() {
    let (service, path) = service("band");
    funded(&service, "maker");
    funded(&service, "taker");
    let place = |account: &str, side: &str, price: i64| {
        service.handle(&request(vec![
            ("op", str_value("place")),
            ("account", str_value(account)),
            ("side", str_value(side)),
            ("item", Value::Int(314)),
            ("qty", Value::Int(1)),
            ("price", Value::Int(price)),
        ]))
    };
    assert_eq!(place("maker", "sell", 2).get("ok").as_bool(), Some(true));
    let traded = place("taker", "buy", 2);
    assert_eq!(traded.get("fills").as_arr().map(|fills| fills.len()), Some(1), "crossed at 2: {traded:?}");
    crate::market_test_support::settle_matched(&service);
    let quote = service.handle(&request(vec![("op", str_value("quote")), ("item", Value::Int(314))]));
    assert_eq!(quote.get("reference").as_i64(), Some(2));
    assert_eq!(quote.get("traded").as_bool(), Some(true));
    assert_eq!(place("taker", "buy", 11).get("ok").as_bool(), Some(false), "11 > 5x the traded 2");
    assert_eq!(place("taker", "buy", 10).get("ok").as_bool(), Some(true), "10 = 5x stays inside");
    cleanup(&path);
}

/// PRD3 lot 2: a bounded bid walks up one step per patience window and
/// crosses the resting ask once it reaches it; never past its ceiling.
#[test]
fn bounded_bids_walk_up_and_cross() {
    let (service, path) = service("reprice");
    funded(&service, "reprice_seller");
    funded(&service, "reprice_buyer");
    let ask = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("reprice_seller")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(ask.get("ok").as_bool(), Some(true));
    let bid = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("reprice_buyer")),
        ("side", str_value("buy")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(1)),
        ("ceiling", Value::Int(8)),
        ("patience", Value::Int(1)),
    ]));
    assert_eq!(bid.get("status").as_str(), Some("open"), "1 does not cross 5: {bid:?}");
    let mut wall = 4_000_000_000.0;
    let mut crossed = false;
    for _ in 0..10 {
        wall += 10.0;
        crate::market_reprice::reprice_round(&service, wall, 600.0);
        crate::market_test_support::settle_matched(&service);
        let quote = service.handle(&request(vec![("op", str_value("quote")), ("item", Value::Int(314))]));
        if quote.get("traded").as_bool() == Some(true) {
            assert_eq!(quote.get("reference").as_i64(), Some(5), "crossed at the resting ask");
            crossed = true;
            break;
        }
    }
    assert!(crossed, "the bid walked 1 -> 5 and crossed");
    cleanup(&path);
}

/// Market page (user rule 2026-11-12): every good with an open order is
/// listed, traded or not, with its open-order count; empty books never.
#[test]
fn list_with_orders_shows_every_resting_book() {
    let (service, path) = service("with-orders");
    funded(&service, "lister");
    let list = |service: &MarketService| {
        service.handle(&request(vec![("op", str_value("list")), ("with_orders", Value::Int(1))])).get("items").as_arr().cloned().unwrap_or_default()
    };
    assert!(list(&service).is_empty(), "no order, no row");
    let placed = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("lister")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(5)),
        ("price", Value::Int(9)),
    ]));
    assert_eq!(placed.get("ok").as_bool(), Some(true));
    let rows = list(&service);
    assert_eq!(rows.len(), 1, "the resting ask lists its good: {rows:?}");
    assert_eq!(rows[0].get("name").as_str(), Some("Feather"));
    assert_eq!(rows[0].get("orders").as_i64(), Some(1));
    assert_eq!(rows[0].get("open_qty").as_i64(), Some(5), "open quantity, not order count");
    cleanup(&path);
}

/// AGENTS rule 13: admin never places an order.
#[test]
fn admin_never_trades() {
    let (service, path) = service("admin-rule");
    funded(&service, "admin");
    let refused = service.handle(&request(vec![
        ("op", str_value("place")),
        ("account", str_value("admin")),
        ("side", str_value("sell")),
        ("item", Value::Int(314)),
        ("qty", Value::Int(1)),
        ("price", Value::Int(5)),
    ]));
    assert_eq!(refused.get("ok").as_bool(), Some(false), "{refused:?}");
    cleanup(&path);
}
