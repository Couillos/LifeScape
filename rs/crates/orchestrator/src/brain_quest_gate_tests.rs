//! Tests of `brain_quest_gate.rs` (live 2026-11-14, bh99tw2f: ten air
//! talismans banked, all ten listed, Rune Mysteries looped for hours).

use lifescape_json::{object, str_value, Value};
use lifescape_planner::QuestShortfall;

use super::{gate_move, release_listings, release_plan, reserved_ids, GateMove};
use crate::market_seed::CatalogueItem;
use crate::market_service::MarketService;

const AIR_TALISMAN: i64 = 1438;

fn service(tag: &str) -> (MarketService, std::path::PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("lifescape-quest-gate-{}-{tag}-{stamp}.db", std::process::id()));
    let catalogue = vec![
        CatalogueItem { id: AIR_TALISMAN, name: "Air talisman".to_string(), shop_price: 5, alch_value: 2 },
        CatalogueItem { id: 995, name: "Coins".to_string(), shop_price: 1, alch_value: 0 },
    ];
    (MarketService::open(&path, catalogue).expect("opens"), path)
}

fn sell(service: &MarketService, account: &str, qty: i64) -> Value {
    service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value("sell")),
        ("item", Value::Int(AIR_TALISMAN)),
        ("qty", Value::Int(qty)),
        ("price", Value::Int(5)),
        ("floor", Value::Int(3)),
    ]))
}

fn listed(service: &MarketService, account: &str) -> Vec<i64> {
    service
        .lock_store()
        .account_orders(account)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.remaining > 0 && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched))
        .map(|row| row.remaining)
        .collect()
}

/// The quest's talisman never goes on sale, and the listing that already
/// locked it is cut by one, its nine others re-listed at its price.
#[test]
fn a_quest_object_is_never_listed_and_its_listing_is_cut() {
    let (service, path) = service("talisman");
    let account = "bh99tw2f";
    service.lock_engine().ledger.sync_account(account, 0, &[(AIR_TALISMAN, 10)]);
    assert_eq!(sell(&service, account, 10).get("ok").as_bool(), Some(true), "before the quest: all ten sell");
    service.set_quest_reserved(account, reserved_ids(&[("air_talisman".to_string(), 1)]));
    let notes = release_listings(&service, account, "air_talisman", 0, 1);
    assert!(notes.iter().any(|note| note.contains("cut by 1")), "{notes:?}");
    assert_eq!(listed(&service, account), vec![9], "nine still for sale");
    let refused = sell(&service, account, 1);
    assert_eq!(refused.get("ok").as_bool(), Some(false), "the reserved unit never lists: {refused:?}");
    service.set_quest_reserved(account, Default::default());
    assert_eq!(sell(&service, account, 1).get("ok").as_bool(), Some(true), "quest closed: it sells again");
    let _ = std::fs::remove_file(&path);
}

/// Newest listing first, only what the need lacks.
#[test]
fn release_cuts_only_what_the_quest_lacks() {
    assert_eq!(release_plan(&[(101, 10)], 0, 1), vec![(101, 1)]);
    assert_eq!(release_plan(&[(101, 3), (140, 2)], 0, 4), vec![(140, 2), (101, 2)]);
    assert!(release_plan(&[(101, 10)], 1, 1).is_empty(), "one free unit is enough");
    assert_eq!(reserved_ids(&[("air_talisman".to_string(), 1)]).get(&AIR_TALISMAN), Some(&1));
}

/// Missing brought items are obtained first; locked units released;
/// unknown holdings wait; nothing short launches.
#[test]
fn the_gate_obtains_releases_or_launches() {
    assert_eq!(gate_move(&[], true), GateMove::Launch);
    let locked = QuestShortfall::Locked { item: "air_talisman".to_string(), need: 1, held: 10, free: 0 };
    assert_eq!(gate_move(&[locked.clone()], true), GateMove::Release(vec![("air_talisman".to_string(), 1, 0)]));
    let missing = QuestShortfall::Missing { item: "ball_of_wool".to_string(), need: 20, held: 5 };
    assert_eq!(gate_move(&[missing.clone(), locked], true), GateMove::Obtain(vec!["ball_of_wool".to_string()]));
    assert_eq!(gate_move(&[missing], false), GateMove::Unknown, "a bank never streamed proves nothing");
}
