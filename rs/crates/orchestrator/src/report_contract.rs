//! Task report JSON, contract 2 (plan rendements R1): written by the
//! worker (`report_value`), read back by the daemon ingest
//! (`parse_contract`) into the store's v2 measure columns. The legacy
//! fields (`activity`, `items` with D45 provenances, `ticks`,
//! `market_pause_ticks`, `levels_before`, `xp`) stay so the current
//! rates aggregation (`rates_service::parse_report`) reads it unchanged.
//!
//! Added by contract 2:
//! - `contract`: 2;
//! - `planner_method` + `method_source` (`node` | `none`: no method was
//!   carried, nothing guessed), `site` ([x, z, level] | null), `target`,
//!   `tool`, `combat_style`, `levels_start` ({skill: level}, every skill);
//! - `work_ticks`, `pause_ticks` (work + pause = `ticks`);
//! - `flows`: the signed balance [{name, cause, qty}] (`flow_causes`);
//! - `units`: {product, input, cast, kill, damage} XP-bearing counters;
//! - `alerts`: [{kind: unattributed | residual, name, qty}].

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use lifescape_json::{object, str_value, Value};

use crate::session_report::TaskReport;

/// Report contract version a worker writes.
pub const CONTRACT: i64 = 2;

fn count_map(map: &BTreeMap<String, i64>) -> Value {
    Value::Obj(map.iter().map(|(name, qty)| (name.clone(), Value::Int(*qty))).collect())
}

/// Report JSON for the daemon push and the session log line.
pub fn report_value(account: &str, report: &TaskReport) -> Value {
    let xp: Vec<Value> = report
        .xp
        .iter()
        .map(|(stat, skill, gain)| object(vec![("stat", Value::Int(i64::from(*stat))), ("skill", str_value(skill)), ("xp", Value::Int(*gain))]))
        .collect();
    let items: Vec<Value> = report
        .items
        .iter()
        .map(|flow| object(vec![("name", str_value(&flow.name)), ("provenance", str_value(flow.provenance)), ("qty", Value::Int(flow.qty))]))
        .collect();
    let flows: Vec<Value> = report
        .flows
        .iter()
        .map(|(name, cause_name, qty)| object(vec![("name", str_value(name)), ("cause", str_value(cause_name)), ("qty", Value::Int(*qty))]))
        .collect();
    let alerts: Vec<Value> = report
        .alerts
        .iter()
        .map(|alert| object(vec![("kind", str_value(alert.kind)), ("name", str_value(&alert.name)), ("qty", Value::Int(alert.qty)), ("detail", str_value(&alert.detail))]))
        .collect();
    let levels = |held: &[(u8, u8)]| {
        Value::Arr(held.iter().map(|(stat, level)| object(vec![("stat", Value::Int(i64::from(*stat))), ("level", Value::Int(i64::from(*level)))])).collect())
    };
    let meta = &report.meta;
    let levels_start: BTreeMap<String, Value> =
        meta.levels_start.iter().map(|(stat, level)| (lifescape_sdk::provenance::skill_name(*stat).to_string(), Value::Int(i64::from(*level)))).collect();
    let work_ticks = report.ticks.saturating_sub(report.market_pause_ticks);
    object(vec![
        ("contract", Value::Int(CONTRACT)),
        ("account", str_value(account)),
        ("activity", str_value(&report.activity)),
        ("node_id", str_value(&report.node_id)),
        ("planner_method", str_value(&meta.planner_method)),
        ("method_source", str_value(if meta.method_source.is_empty() { "none" } else { meta.method_source })),
        (
            "site",
            match meta.site {
                Some((x, z, level)) => Value::Arr(vec![Value::Int(i64::from(x)), Value::Int(i64::from(z)), Value::Int(i64::from(level))]),
                None => Value::Null,
            },
        ),
        ("target", str_value(&meta.target)),
        ("tool", str_value(&meta.tool)),
        ("combat_style", str_value(&meta.combat_style)),
        ("levels_start", Value::Obj(levels_start)),
        ("stop", str_value(&report.stop)),
        ("missing", Value::Arr(report.missing.iter().map(|name| str_value(name)).collect())),
        ("ticks", Value::Int(report.ticks as i64)),
        ("market_pause_ticks", Value::Int(report.market_pause_ticks as i64)),
        ("work_ticks", Value::Int(work_ticks as i64)),
        ("pause_ticks", Value::Int(report.market_pause_ticks as i64)),
        ("bank_opens", Value::Int(report.bank_opens as i64)),
        ("xp", Value::Arr(xp)),
        ("items", Value::Arr(items)),
        ("flows", Value::Arr(flows)),
        (
            "units",
            object(vec![
                ("product", count_map(&report.units.products)),
                ("input", count_map(&report.units.inputs)),
                ("cast", count_map(&report.units.casts)),
                ("kill", count_map(&report.units.kills)),
                ("damage", count_map(&report.units.damage)),
            ]),
        ),
        ("alerts", Value::Arr(alerts)),
        ("levels_before", levels(&report.levels_before)),
        ("levels_after", levels(&report.levels_after)),
        (
            "error",
            match &report.error {
                Some((kind, message)) => object(vec![("type", str_value(kind)), ("message", str_value(message))]),
                None => Value::Null,
            },
        ),
    ])
}

/// Contract-2 fields of a pushed report, for the store's v2 columns.
/// A contract-1 report (no `contract`) parses to `None`: its rows stay
/// measure version 1 (the purge target, D8).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedContract {
    pub planner_method: String,
    pub method_source: String,
    pub site: String,
    pub target: String,
    pub tool: String,
    pub combat_style: String,
    pub work_ticks: i64,
    pub pause_ticks: i64,
    pub levels: Vec<(String, i64)>,
    /// (item, cause, signed quantity).
    pub flows: Vec<(String, String, i64)>,
    /// (unit kind, subject, quantity).
    pub units: Vec<(String, String, i64)>,
    /// (alert kind, item, quantity).
    pub alerts: Vec<(String, String, i64)>,
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).as_str().unwrap_or("").to_string()
}

/// Reads the contract-2 fields; an unknown cause or a contract newer
/// than this build fails loud (never stored half-understood).
pub fn parse_contract(report: &Value) -> Result<Option<ParsedContract>, String> {
    let Some(contract) = report.get("contract").as_i64() else { return Ok(None) };
    if contract != CONTRACT {
        return Err(format!("report contract {contract} unknown (this build reads {CONTRACT})"));
    }
    let site = match report.get("site").as_arr() {
        Some(parts) => parts.iter().map(|part| part.as_i64().unwrap_or(0).to_string()).collect::<Vec<_>>().join(","),
        None => String::new(),
    };
    let levels = report.get("levels_start").as_obj().map(|map| map.iter().map(|(skill, level)| (skill.clone(), level.as_i64().unwrap_or(0))).collect()).unwrap_or_default();
    let mut flows = Vec::new();
    for row in report.get("flows").as_arr().cloned().unwrap_or_default() {
        let cause_name = text(&row, "cause");
        if !crate::flow_causes::cause::ALL.contains(&cause_name.as_str()) {
            return Err(format!("report flow '{}' has unknown cause '{cause_name}'", text(&row, "name")));
        }
        flows.push((text(&row, "name"), cause_name, row.get("qty").as_i64().unwrap_or(0)));
    }
    let mut units = Vec::new();
    if let Some(groups) = report.get("units").as_obj() {
        for (unit, counts) in groups {
            for (subject, qty) in counts.as_obj().cloned().unwrap_or_default() {
                units.push((unit.clone(), subject, qty.as_i64().unwrap_or(0)));
            }
        }
    }
    let alerts = report.get("alerts").as_arr().cloned().unwrap_or_default().iter().map(|row| (text(row, "kind"), text(row, "name"), row.get("qty").as_i64().unwrap_or(0))).collect();
    Ok(Some(ParsedContract {
        planner_method: text(report, "planner_method"),
        method_source: text(report, "method_source"),
        site,
        target: text(report, "target"),
        tool: text(report, "tool"),
        combat_style: text(report, "combat_style"),
        work_ticks: report.get("work_ticks").as_i64().unwrap_or(0),
        pause_ticks: report.get("pause_ticks").as_i64().unwrap_or(0),
        levels,
        flows,
        units,
        alerts,
    }))
}
