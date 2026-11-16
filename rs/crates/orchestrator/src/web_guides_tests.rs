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
    for key in ["profit_6000", "xp_6000", "prereqs", "stages", "other_drops", "accessible", "band", "tool"] {
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
}
