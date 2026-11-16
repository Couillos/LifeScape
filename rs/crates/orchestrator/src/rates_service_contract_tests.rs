//! Snapshot tests of the contract-2 path (plan rendements R2): only
//! contract-2 reports feed the estimator, the file round-trips, the
//! store rebuild equals the live ingest, the page serves every key.

use super::super::snapshot_io::{rebuild_from_store, snapshot_to_value};
use super::super::*;
use super::parsed_fishing;
use crate::rates_estimator::YieldKey;
use crate::report_contract::ParsedContract;
use lifescape_store::Store;

fn contract(qty: i64, ticks: i64) -> ParsedContract {
    ParsedContract {
        planner_method: "gather:fishing/raw_shrimp".to_string(),
        method_source: "node".to_string(),
        site: "3087,3227,0".to_string(),
        target: "raw_shrimp".to_string(),
        tool: "small_fishing_net".to_string(),
        combat_style: String::new(),
        work_ticks: ticks,
        pause_ticks: 0,
        levels: vec![("fishing".to_string(), 8)],
        flows: vec![("Raw shrimps".to_string(), "product".to_string(), qty)],
        units: vec![("product".to_string(), "Raw shrimps".to_string(), qty)],
        alerts: Vec::new(),
    }
}

fn exact() -> YieldKey {
    YieldKey { method: "gather:fishing/raw_shrimp".to_string(), style: String::new(), site: "3087,3227,0".to_string(), tool: "small_fishing_net".to_string(), band: 6 }
}

/// Contract-1 reports (method = script) never reach the estimator;
/// contract-2 ones land under their D2 key and its roll-ups.
#[test]
fn only_contract_two_feeds_the_estimator() {
    let mut snapshot = Snapshot::new();
    assert!(snapshot.record(&parsed_fishing(300, 6000, 8), None, 1).is_none());
    assert!(snapshot.book().buckets.is_empty(), "v1 never aggregated");
    let admitted = snapshot.record(&parsed_fishing(300, 6000, 8), Some(&contract(100, 900)), 1);
    assert!(matches!(admitted, Some(crate::rates_measure::Admission::Rated(_, _))), "{admitted:?}");
    assert_eq!(snapshot.book().get(&exact()).map(|bucket| bucket.runs), Some(1));
    assert_eq!(snapshot.book().buckets.len(), 3);
    let mut tutorial = parsed_fishing(300, 6000, 8);
    tutorial.method = "tutorial".to_string();
    assert!(snapshot.record(&tutorial, Some(&contract(100, 900)), 1).is_none(), "tutorial feeds nothing");
    let mut market = parsed_fishing(300, 6000, 8);
    market.method = "market:make".to_string();
    snapshot.record(&market, Some(&contract(100, 900)), 1);
    assert_eq!(snapshot.book().get(&exact()).map(|bucket| bucket.runs), Some(1), "trading feeds the book only");
}

/// The file round-trips the buckets; an older schema reads as missing
/// (rebuilt from the store).
#[test]
fn snapshot_file_round_trips_schema_three() {
    let mut snapshot = Snapshot::new();
    for qty in [100, 120, 140] {
        snapshot.record(&parsed_fishing(qty, 1200, 8), Some(&contract(qty, 1200)), 7);
    }
    let text = lifescape_json::stringify(&snapshot_to_value(&snapshot));
    let reloaded = snapshot_from_json(&text).expect("schema 3 loads");
    assert_eq!(reloaded.book, snapshot.book);
    assert!(snapshot_from_json("{\"schema\":2,\"yields\":{}}").is_none(), "older schema: rebuild");
    let size = text.len();
    for qty in 0..200 {
        snapshot.record(&parsed_fishing(qty, 1200, 8), Some(&contract(qty, 1200)), 8);
    }
    let grown = lifescape_json::stringify(&snapshot_to_value(&snapshot)).len();
    assert!(grown < size + 64, "size bounded by keys, not runs: {size} -> {grown}");
}

/// The store rebuild replays the contract-2 reports through the same
/// admission: the rebuilt buckets equal the live ones.
#[test]
fn snapshot_rebuilds_from_the_store() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let mut live = Snapshot::new();
    for (index, qty) in [100, 120, 140, 160].iter().enumerate() {
        let report_id = store.record_task_report(&lifescape_store::NewTaskReport {
            account: "t01".to_string(),
            node_id: "n1".to_string(),
            method: "fishing".to_string(),
            script: "fishing".to_string(),
            ticks: 1200,
            stop: "budget_exhausted".to_string(),
            context_level: 8,
            clock: 50 + index as i64,
        })?;
        let parsed = contract(*qty, 1200);
        let measure = lifescape_store::ReportMeasure {
            planner_method: parsed.planner_method.clone(),
            method_source: parsed.method_source.clone(),
            site: parsed.site.clone(),
            target: parsed.target.clone(),
            tool: parsed.tool.clone(),
            combat_style: parsed.combat_style.clone(),
            work_ticks: parsed.work_ticks,
            pause_ticks: parsed.pause_ticks,
            levels: parsed.levels.clone(),
            flows: parsed.flows.clone(),
            units: parsed.units.clone(),
            alerts: parsed.alerts.clone(),
        };
        store.record_report_measure(report_id, &measure)?;
        live.record(&parsed_fishing(*qty, 1200, 8), Some(&parsed), 50 + index as i64);
    }
    // A contract-1 row never replays.
    store.record_task_report(&lifescape_store::NewTaskReport {
        account: "t01".to_string(),
        node_id: "n2".to_string(),
        method: "fishing".to_string(),
        script: "fishing".to_string(),
        ticks: 1200,
        stop: "budget_exhausted".to_string(),
        context_level: 8,
        clock: 99,
    })?;
    let rebuilt = rebuild_from_store(&store).map_err(lifescape_store::Error::Invalid)?;
    assert_eq!(rebuilt.book, live.book);
    Ok(())
}

/// The page serves every measured key: signed flows, units and derived
/// XP with the key and the confidence.
#[test]
fn payload_serves_every_key() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let mut snapshot = Snapshot::new();
    for qty in [100, 120, 140] {
        snapshot.record(&parsed_fishing(qty, 1200, 8), Some(&contract(qty, 1200)), 7);
    }
    let payload = rates_payload(&store, &snapshot, std::collections::BTreeMap::new());
    let rows = payload.get("rates").as_arr().cloned().unwrap_or_default();
    let exact_row = rows.iter().find(|row| row.get("site").as_str() == Some("3087,3227,0") && row.get("item").as_str() == Some("Raw shrimps")).expect("exact row");
    assert_eq!(exact_row.get("runs").as_i64(), Some(3));
    assert_eq!(exact_row.get("thin").as_bool(), Some(false));
    assert_eq!(exact_row.get("cause").as_str(), Some("product"));
    let keys = |row: &lifescape_json::Value| {
        let mut out: Vec<String> = row.as_obj().map(|map| map.keys().cloned().collect()).unwrap_or_default();
        out.sort();
        out
    };
    assert_eq!(keys(exact_row), crate::api_shapes::rate_row_columns(crate::api_shapes::YIELD_COLUMNS), "frozen columns");
    let xp = payload.get("xp").as_arr().cloned().unwrap_or_default();
    assert!(xp.iter().any(|row| row.get("skill").as_str() == Some("fishing") && row.get("per_6000").as_f64().unwrap_or(0.0) > 0.0));
    let units = payload.get("units").as_arr().cloned().unwrap_or_default();
    assert_eq!(units.first().map(keys), Some(crate::api_shapes::rate_row_columns(crate::api_shapes::UNIT_COLUMNS)));
    assert_eq!(xp.first().map(keys), Some(crate::api_shapes::rate_row_columns(crate::api_shapes::XP_COLUMNS)));
    assert!(payload.get("profit").as_arr().is_some_and(|rows| rows.len() == 1), "one profit row per method band");
    Ok(())
}

/// R2.1: the measures view publishes each measured exact key with ITS
/// own sums: no roll-up row, no thin key showing another key's value,
/// rare pickups folded into one valued "other drops" line per key.
#[test]
fn measures_publish_no_copies_and_fold_rare_drops() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    let mut snapshot = Snapshot::new();
    for qty in [100, 120, 140] {
        snapshot.record(&parsed_fishing(qty, 1200, 8), Some(&contract(qty, 1200)), 7);
    }
    // A thin key (one run) on another site, with a rare pickup and a trade.
    let mut thin = contract(150, 7200);
    thin.site = "3100,3230,0".to_string();
    thin.flows.push(("Bronze med helm".to_string(), "ground_spawn".to_string(), 1));
    thin.flows.push(("Coins".to_string(), "traded".to_string(), 500));
    snapshot.record(&parsed_fishing(150, 7200, 8), Some(&thin), 7);
    let payload = rates_payload(&store, &snapshot, std::collections::BTreeMap::new());
    let rows = payload.get("rates").as_arr().cloned().unwrap_or_default();
    assert!(rows.iter().all(|row| row.get("site").as_str() != Some("*") && row.get("tool").as_str() != Some("*")), "roll-ups stay internal");
    let shrimps: Vec<(String, f64, bool)> = rows
        .iter()
        .filter(|row| row.get("item").as_str() == Some("Raw shrimps"))
        .map(|row| (row.get("site").as_str().unwrap_or("").to_string(), row.get("per_6000").as_f64().unwrap_or(0.0), row.get("thin").as_bool().unwrap_or(false)))
        .collect();
    assert_eq!(shrimps.len(), 2, "one line per measured key: {shrimps:?}");
    let thin_row = shrimps.iter().find(|(site, _, _)| site == "3100,3230,0").expect("thin key");
    assert!(thin_row.2 && (thin_row.1 - 125.0).abs() < 1e-6, "the thin key shows its own 150 / 7200 ticks: {thin_row:?}");
    assert!(rows.iter().all(|row| row.get("cause").as_str() != Some("traded")), "trades never rate");
    let other = rows.iter().find(|row| row.get("item").as_str().is_some_and(|item| item.starts_with("other drops"))).expect("folded drops");
    assert_eq!(other.get("site").as_str(), Some("3100,3230,0"));
    assert!(rows.iter().all(|row| row.get("item").as_str() != Some("Bronze med helm")), "the rare pickup has no line of its own");
    Ok(())
}

/// R2.1 migration: the store keeps planner method, style and target; the
/// rebuild derives the activity again, so drop farms of one fight that
/// were stored under the objects wanted land in one bucket.
#[test]
fn rebuild_keys_stored_drop_farms_by_their_fight() -> Result<(), lifescape_store::Error> {
    let store = Store::open_in_memory()?;
    for (index, wanted) in ["farm:bronze_arrow", "farm:naturerune", "fight:melee/goblins"].iter().enumerate() {
        let report_id = store.record_task_report(&lifescape_store::NewTaskReport {
            account: "t01".to_string(),
            node_id: format!("n{index}"),
            method: "melee".to_string(),
            script: "melee".to_string(),
            ticks: 1200,
            stop: "budget_exhausted".to_string(),
            context_level: 8,
            clock: 50 + index as i64,
        })?;
        let measure = lifescape_store::ReportMeasure {
            planner_method: wanted.to_string(),
            method_source: "node".to_string(),
            site: "3183,3244,0".to_string(),
            target: "goblins".to_string(),
            tool: "bronze_sword".to_string(),
            combat_style: "melee:attack".to_string(),
            work_ticks: 1200,
            pause_ticks: 0,
            levels: vec![("attack".to_string(), 7)],
            flows: vec![("Bones".to_string(), "loot".to_string(), 10), ("Coins".to_string(), "traded".to_string(), -40)],
            units: vec![("kill".to_string(), "goblin".to_string(), 10), ("damage".to_string(), "goblin".to_string(), 50)],
            alerts: Vec::new(),
        };
        store.record_report_measure(report_id, &measure)?;
    }
    let rebuilt = rebuild_from_store(&store).map_err(lifescape_store::Error::Invalid)?;
    let exact: Vec<&YieldKey> = rebuilt.book().buckets.keys().filter(|key| !key.is_rollup()).collect();
    assert_eq!(exact.len(), 1, "{exact:?}");
    assert_eq!(exact[0].method, "fight:melee/goblins");
    let bucket = rebuilt.book().get(exact[0]).expect("bucket");
    assert_eq!(bucket.runs, 3);
    assert!(bucket.flows.keys().all(|(_, cause)| cause != "traded"), "trades stay in the store");
    Ok(())
}
