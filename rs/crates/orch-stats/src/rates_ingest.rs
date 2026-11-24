#![forbid(unsafe_code)]
//! Rates ingest (lot rendements): the daemon-side store write of one
//! pushed task report. Moved out of `jobs` (fleet layer) into the stats
//! layer, which owns the report parsing and the rates snapshot (layering
//! step A3, 2026-11-23). Pure code motion.

use lifescape_json::Value;

/// Store file the daemon alone opens (the same file the market service
/// holds): one short-lived connection per report, WAL-safe.
fn rates_db_path() -> std::path::PathBuf {
    crate::paths::store_db()
}

/// Parses one pushed TaskReport and appends its raw rows to the store:
/// one `task_reports` row plus one `task_yields` line per item flow and
/// per XP gain. Every provenance is stored raw (shop and trade lines
/// included); the rates snapshot excludes them at aggregation time.
/// Returns the parsed report for the snapshot caller.
pub fn ingest_report_into(
    store: &lifescape_store::Store,
    report: &Value,
) -> Result<crate::rates_service::ParsedReport, String> {
    let (parsed, contract) = parse_for_ingest(report)?;
    ingest_parsed_into(store, &parsed, contract)?;
    Ok(parsed)
}

/// The pure half of an ingest: the report and its contract, parsed and
/// validated before any write (and, in the daemon, before the writer's
/// lock is taken).
fn parse_for_ingest(
    report: &Value,
) -> Result<(crate::rates_service::ParsedReport, Option<crate::report_contract::ParsedContract>), String> {
    let parsed = crate::rates_service::parse_report(report)?;
    // Contract 2 (plan rendements R1): parsed before any write, so an
    // unknown cause or contract never leaves half a report behind.
    let contract = crate::report_contract::parse_contract(report)?;
    Ok((parsed, contract))
}

/// The DB half: the rows of an already parsed report, in one transaction.
fn ingest_parsed_into(
    store: &lifescape_store::Store,
    parsed: &crate::rates_service::ParsedReport,
    contract: Option<crate::report_contract::ParsedContract>,
) -> Result<(), String> {
    // One transaction for the report, its lines and its measure: one
    // journal sync instead of one per statement, and a failure midway
    // leaves no half report.
    let owned = store.begin_batch().map_err(|error| format!("rates store batch: {error}"))?;
    let written = ingest_report_rows(store, parsed, contract);
    if owned {
        match &written {
            Ok(_) => store.commit_batch().map_err(|error| format!("rates store commit: {error}"))?,
            Err(_) => store.rollback_batch(),
        }
    }
    written
}

fn ingest_report_rows(
    store: &lifescape_store::Store,
    parsed: &crate::rates_service::ParsedReport,
    contract: Option<crate::report_contract::ParsedContract>,
) -> Result<(), String> {
    let report_id = store
        .record_task_report(&lifescape_store::NewTaskReport {
            account: parsed.account.clone(),
            node_id: parsed.node_id.clone(),
            method: parsed.method.clone(),
            script: parsed.script.clone(),
            ticks: parsed.wall_ticks,
            stop: parsed.stop.clone(),
            context_level: parsed.level,
            clock: crate::clock::now_secs() as i64,
        })
        .map_err(|error| format!("rates store report: {error}"))?;
    let mut lines = 0;
    for flow in &parsed.items {
        store
            .record_task_yield(&lifescape_store::NewTaskYield {
                report_id,
                item_name: flow.name.clone(),
                provenance: flow.provenance.clone(),
                quantity: flow.quantity,
                skill: String::new(),
                xp: 0,
            })
            .map_err(|error| format!("rates store yield: {error}"))?;
        lines += 1;
    }
    for gain in &parsed.xp {
        store
            .record_task_yield(&lifescape_store::NewTaskYield {
                report_id,
                item_name: String::new(),
                provenance: "produced".to_string(),
                quantity: 0,
                skill: gain.skill.clone(),
                xp: gain.amount,
            })
            .map_err(|error| format!("rates store xp: {error}"))?;
        lines += 1;
    }
    if let Some(contract) = contract {
        let measure = lifescape_store::ReportMeasure {
            planner_method: contract.planner_method,
            method_source: contract.method_source,
            site: contract.site,
            target: contract.target,
            tool: contract.tool,
            combat_style: contract.combat_style,
            work_ticks: contract.work_ticks,
            pause_ticks: contract.pause_ticks,
            levels: contract.levels,
            flows: contract.flows,
            units: contract.units,
            alerts: contract.alerts,
        };
        store.record_report_measure(report_id, &measure).map_err(|error| format!("rates store measure: {error}"))?;
    }
    crate::logln!(
        "rates: stored report {report_id} {} {} ({} lines)",
        parsed.account, parsed.method, lines
    );
    Ok(())
}

/// File-backed ingest for the supervisor thread: opens the daemon store
/// (retrying through SQLite lock contention with the market service),
/// stores the raw rows, then folds the report into the persisted rates
/// snapshot. Loud error, never a silent drop.
pub fn ingest_report_persisted(report: &Value) -> Result<(), String> {
    // Parsed and validated once, before the writer's lock (a bad report
    // never reaches the store, and parsing never holds the market up).
    let (parsed, contract) = parse_for_ingest(report)?;
    let mut last_error = "rates store: no attempt".to_string();
    for _attempt in 0..20 {
        // Through the process's single writer when the daemon registered
        // one (grouped commit, acknowledged after it), else a pooled
        // connection that commits by itself.
        match crate::store_writer::write_to(&rates_db_path(), |store| ingest_parsed_into(store, &parsed, contract.clone())) {
            Ok(written) => {
                written?;
                // Contract 2 feeds the estimator (parsed and validated
                // above, before any write); contract 1 never does.
                crate::rates_service::record_snapshot(&parsed, contract.as_ref(), crate::rates_service::now_secs())?;
                return Ok(());
            }
            Err(error) => {
                last_error = error.to_string();
                if !(last_error.contains("locked") || last_error.contains("busy")) {
                    return Err(format!("rates store open: {last_error}"));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err(format!("rates store open, retries exhausted: {last_error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json::{object, str_value};

    fn fishing_report() -> Value {
        object(vec![
            ("account", str_value("t01")),
            ("activity", str_value("fishing")),
            ("node_id", str_value("n9")),
            ("stop", str_value("budget_exhausted")),
            ("ticks", Value::Int(6000)),
            ("market_pause_ticks", Value::Int(0)),
            (
                "xp",
                Value::Arr(vec![object(vec![
                    ("stat", Value::Int(10)),
                    ("skill", str_value("fishing")),
                    ("xp", Value::Int(4000)),
                ])]),
            ),
            (
                "items",
                Value::Arr(vec![
                    object(vec![
                        ("name", str_value("Raw shrimps")),
                        ("provenance", str_value("produced")),
                        ("qty", Value::Int(300)),
                    ]),
                    object(vec![
                        ("name", str_value("Fishing bait")),
                        ("provenance", str_value("shop_bought")),
                        ("qty", Value::Int(100)),
                    ]),
                    object(vec![
                        ("name", str_value("Coins")),
                        ("provenance", str_value("traded")),
                        ("qty", Value::Int(500)),
                    ]),
                ]),
            ),
            (
                "levels_before",
                Value::Arr(vec![object(vec![("stat", Value::Int(10)), ("level", Value::Int(8))])]),
            ),
            ("levels_after", Value::Arr(vec![])),
        ])
    }

    /// Ingest stores every line raw (farm, shop and trade alike) with
    /// the wall ticks and the context level; aggregation filters later.
    #[test]
    fn ingest_stores_all_lines_raw() {
        let store = lifescape_store::Store::open_in_memory().expect("memory store");
        let parsed = super::ingest_report_into(&store, &fishing_report()).expect("ingests");
        assert_eq!((parsed.method.as_str(), parsed.level, parsed.band), ("fishing", 8, 6));
        let aggregates = store.aggregate_yields().expect("aggregates");
        let shrimp = aggregates
            .iter()
            .find(|line| line.item_name == "Raw shrimps")
            .expect("shrimp aggregate");
        assert_eq!((shrimp.total_quantity, shrimp.report_count), (300, 1));
        // Shop and trade lines are stored too: raw rows are kept, the
        // snapshot (not the store) excludes them from rates.
        assert!(aggregates.iter().any(|line| line.provenance == "shop_bought"), "shop line kept");
        assert!(aggregates.iter().any(|line| line.provenance == "traded"), "trade line kept");
        assert_eq!(aggregates.iter().map(|line| line.report_count).sum::<i64>(), 4, "one report, four lines");
    }

    /// A contract-2 report stores its measure beside the legacy lines;
    /// the legacy aggregation reads the same report unchanged.
    #[test]
    fn ingest_stores_the_contract_two_measure() {
        let store = lifescape_store::Store::open_in_memory().expect("memory store");
        let mut report = fishing_report();
        if let Value::Obj(map) = &mut report {
            map.insert("contract".to_string(), Value::Int(2));
            map.insert("planner_method".to_string(), str_value("gather:fishing/raw_shrimp"));
            map.insert("method_source".to_string(), str_value("node"));
            map.insert("work_ticks".to_string(), Value::Int(6000));
            map.insert("flows".to_string(), Value::Arr(vec![object(vec![("name", str_value("Raw shrimps")), ("cause", str_value("product")), ("qty", Value::Int(300))])]));
            map.insert("units".to_string(), object(vec![("product", object(vec![("Raw shrimps", Value::Int(300))]))]));
        }
        let parsed = super::ingest_report_into(&store, &report).expect("ingests");
        assert_eq!(parsed.method, "fishing", "the legacy key stays the script");
        let measure = store.report_measure(1).expect("reads").expect("measure 2");
        assert_eq!((measure.planner_method.as_str(), measure.work_ticks), ("gather:fishing/raw_shrimp", 6000));
        assert_eq!(measure.flows, vec![("Raw shrimps".to_string(), "product".to_string(), 300)]);
        assert_eq!(measure.units, vec![("product".to_string(), "Raw shrimps".to_string(), 300)]);
        // An unknown cause fails before any row lands.
        if let Value::Obj(map) = &mut report {
            map.insert("flows".to_string(), Value::Arr(vec![object(vec![("name", str_value("X")), ("cause", str_value("magic")), ("qty", Value::Int(1))])]));
        }
        assert!(super::ingest_report_into(&store, &report).is_err());
        assert_eq!(store.v1_measure_counts().expect("counts").reports, 0, "nothing half-stored");
    }

    /// Empty account or method fails loud, never stores a phantom row.
    #[test]
    fn ingest_rejects_empty_identity() {
        let store = lifescape_store::Store::open_in_memory().expect("memory store");
        assert!(super::ingest_report_into(&store, &object(vec![])).is_err(), "empty report fails");
        let mut no_account = fishing_report();
        if let Value::Obj(map) = &mut no_account {
            map.insert("account".to_string(), str_value(""));
        }
        assert!(super::ingest_report_into(&store, &no_account).is_err(), "empty account fails");
        assert!(store.aggregate_yields().expect("aggregates").is_empty(), "nothing stored");
    }
}
