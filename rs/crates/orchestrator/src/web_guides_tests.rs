//! Guides API on an in-memory store and a fabricated snapshot (never
//! `run/`): the measured fight of a drop farm prices its whole loot, the
//! filters apply, the JSON carries the guide columns.

use super::*;
use crate::rates_service::Snapshot;
use crate::report_contract::ParsedContract;

fn fight(qty: i64) -> ParsedContract {
    ParsedContract {
        planner_method: "farm:feather".to_string(),
        method_source: "node".to_string(),
        site: "3235,3295,0".to_string(),
        target: "chickens".to_string(),
        tool: "bronze_sword".to_string(),
        combat_style: "melee:attack".to_string(),
        work_ticks: 1200,
        pause_ticks: 0,
        levels: vec![("attack".to_string(), 3)],
        flows: vec![("Feather".to_string(), "loot".to_string(), qty), ("Bones".to_string(), "loot".to_string(), 10), ("Coins".to_string(), "traded".to_string(), 900)],
        units: vec![("kill".to_string(), "chicken".to_string(), 10), ("damage".to_string(), "chicken".to_string(), 30)],
        alerts: Vec::new(),
    }
}

fn parsed() -> crate::rates_service::ParsedReport {
    crate::rates_service::ParsedReport {
        account: "t01".to_string(),
        node_id: "n1".to_string(),
        method: "farm:feather".to_string(),
        script: "melee".to_string(),
        wall_ticks: 1200,
        pause_ticks: 0,
        stop: "budget_exhausted".to_string(),
        level: 3,
        band: 1,
        items: Vec::new(),
        xp: Vec::new(),
    }
}

/// The money guide reads the fight the drop farms measured (its whole
/// loot priced, trades never), with its confidence; filters apply.
#[test]
fn money_guide_prices_the_measured_fight() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let mut snapshot = Snapshot::new();
    for qty in [50, 60, 70] {
        snapshot.record(&parsed(), Some(&fight(qty)), 9);
    }
    let filter = GuideFilter { skill: Some("attack".to_string()), level: Some(3), ..GuideFilter::default() };
    let guide = build_guide(&store, &snapshot, GuideKind::Money, &filter).map_err(lifescape_store::Error::Invalid)?;
    let rows = guide.get("rows").as_arr().cloned().unwrap_or_default();
    let measured = rows
        .iter()
        .find(|row| row.get("method").as_str() == Some("fight:melee/chickens") && row.get("site").as_str() == Some("3235,3295,0"))
        .expect("the measured site has its line");
    assert_eq!(measured.get("family").as_str(), Some("farm"));
    assert_eq!(measured.get("confidence").get("measured").as_bool(), Some(true));
    let flows = measured.get("flows").as_arr().cloned().unwrap_or_default();
    assert!(flows.iter().any(|line| line.get("item").as_str() == Some("feather")), "{flows:?}");
    assert!(flows.iter().all(|line| line.get("cause").as_str() != Some("traded")));
    assert!(rows.iter().all(|row| row.get("prereqs").get("levels").get("attack").as_i64().unwrap_or(0) <= 3 || !row.get("accessible").as_bool().unwrap_or(true)));
    for key in ["profit_6000", "xp_6000", "prereqs", "stages", "routes", "other_drops", "accessible", "band", "tool"] {
        assert!(!matches!(measured.get(key), Value::Null), "{key} column");
    }
    Ok(())
}

/// The query string sets the filter; empty values are no filter.
#[test]
fn query_sets_the_filter() {
    let params = crate::web::parse_query("skill=mining&level=41&family=&quests=doric,runemysteries");
    let filter = filter_of(&params);
    assert_eq!(filter.skill.as_deref(), Some("mining"));
    assert_eq!(filter.level, Some(41));
    assert_eq!(filter.family, None);
    assert_eq!(filter.quests_done, vec!["doric".to_string(), "runemysteries".to_string()]);
    assert!(filter.held.is_empty(), "the general guide holds nothing");
    let held = filter_of(&crate::web::parse_query("level=30&held=iron_ore:50,coal:500"));
    assert_eq!(held.held, vec![("iron_ore".to_string(), 50.0), ("coal".to_string(), 500.0)]);
}

/// D12: a never-traded object's quote carries no current price, only its
/// resting book (a lone bid for one unit is one unit deep); a traded one
/// carries its current price.
#[test]
fn never_traded_quote_carries_its_book_depth() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    store.upsert_item(&lifescape_store::Item { item_id: 1097, name: "Leather chaps".to_string(), shop_price: 20, alch_value: 8 })?;
    store.place_order(&lifescape_store::NewOrder { account: "t01".to_string(), side: lifescape_store::OrderSide::Buy, item_id: 1097, quantity: 1, price: 15411, clock: 1, ..Default::default() })?;
    let quote = quote_of(&store, "Leather chaps");
    assert!(!quote.traded);
    assert_eq!(quote.bids, vec![(15411.0, 1.0)]);
    assert!(quote.asks.is_empty());
    // One fill at 400: traded on one fill (thin), the bid clamps the
    // price (D12) and stays in the quote for its one unit.
    store.record_execution(&lifescape_store::NewExecution { item_id: 1097, quantity: 1, price: 400, clock: 2, server_tick: 2, taker_side: Some(lifescape_store::OrderSide::Buy) })?;
    let quote = quote_of(&store, "Leather chaps");
    assert!(quote.traded);
    assert_eq!((quote.median, quote.fills, quote.fill_units), (400.0, 1, 1.0));
    assert_eq!(quote.price, 15411.0, "D12 clamp by the bid");
    assert_eq!(quote.bids, vec![(15411.0, 1.0)]);
    Ok(())
}

/// The shops' window the engine reads (§2): the base stock and the
/// restock of one window, cheapest first, never an unlimited shop.
#[test]
fn shop_window_is_finite_and_cheapest_first() {
    let steps = super::shop_window("feather");
    let units: f64 = steps.iter().map(|(_, units)| units).sum();
    assert!(units.is_finite() && units >= 7000.0, "{steps:?}");
    assert!(steps.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert!(super::shop_window("no_such_object").is_empty());
}
