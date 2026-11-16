#![forbid(unsafe_code)]
//! Snapshot persistence of the rates: the JSON file (schema 4: the
//! estimator's buckets and the merchant book), its reload, and the
//! rebuild from the store when the file is missing, corrupt or older.
//!
//! Rebuild: the trading positions re-cut from the raw log (as before);
//! the estimator replays every contract-2 report of the store in order
//! through the same admission as the live ingest (`rates_measure`), so
//! the rebuilt buckets equal the live ones (decay included). Contract-1
//! rows are never read (D8).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_store::Store;

use super::trading;
use super::Snapshot;
use crate::rates_estimator::{Bucket, YieldBook, YieldKey};

/// Snapshot schema: 4 = per-ACTIVITY estimator buckets (R2.1: a drop
/// farm keyed by its fight, yield causes only); 3 keyed by the planner
/// method with every cause. Older files are dropped and rebuilt from the
/// store (the reports keep planner method, style, target and site: the
/// activity is derived again at the replay).
const SNAPSHOT_SCHEMA: i64 = 4;
/// Store reports read per page during a rebuild.
const REBUILD_PAGE: i64 = 2000;

/// Snapshot file beside the staging log: rewritten by `rates_flush`,
/// reloaded when the daemon restarts.
pub fn snapshot_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_RATES_PATH") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    crate::paths::run_dir().join("rates-snapshot.json")
}

pub(super) fn global_snapshot() -> &'static Mutex<Arc<Snapshot>> {
    static SNAPSHOT: OnceLock<Mutex<Arc<Snapshot>>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| Mutex::new(Arc::new(load_snapshot_file())))
}

fn load_snapshot_file() -> Snapshot {
    let path = snapshot_path();
    if let Some(snapshot) = std::fs::read_to_string(&path).ok().and_then(|text| snapshot_from_json(&text)) {
        return snapshot;
    }
    let Ok(store) = Store::open(&crate::paths::store_db()) else {
        crate::logln!("rates: snapshot {} unreadable and store closed: empty estimator", path.display());
        return Snapshot::new();
    };
    match rebuild_from_store(&store) {
        Ok(snapshot) => {
            crate::rates_flush::mark_dirty();
            snapshot
        }
        Err(error) => {
            crate::logln!("rates: rebuild from the store failed: {error}");
            Snapshot::new()
        }
    }
}

/// The whole snapshot from the store: trading positions plus every
/// contract-2 report replayed through the live admission.
pub fn rebuild_from_store(store: &Store) -> Result<Snapshot, String> {
    let mut snapshot = trading::rebuild_from_store(store)?;
    let mut after = 0;
    let mut replayed = 0usize;
    loop {
        let page = store.measures_after(after, REBUILD_PAGE).map_err(|error| format!("rates rebuild measures: {error}"))?;
        let Some(last) = page.last() else { break };
        after = last.report_id;
        for stored in &page {
            let contract = crate::report_contract::ParsedContract {
                planner_method: stored.measure.planner_method.clone(),
                method_source: stored.measure.method_source.clone(),
                site: stored.measure.site.clone(),
                target: stored.measure.target.clone(),
                tool: stored.measure.tool.clone(),
                combat_style: stored.measure.combat_style.clone(),
                work_ticks: stored.measure.work_ticks,
                pause_ticks: stored.measure.pause_ticks,
                levels: stored.measure.levels.clone(),
                flows: stored.measure.flows.clone(),
                units: stored.measure.units.clone(),
                alerts: stored.measure.alerts.clone(),
            };
            if crate::rates_service::is_trading_method(&contract.planner_method) {
                continue;
            }
            let admission = crate::rates_measure::admit(&contract, &stored.stop, stored.clock);
            if !matches!(admission, crate::rates_measure::Admission::Skipped(_)) {
                snapshot.apply(&admission, &stored.account);
            }
            replayed += 1;
        }
    }
    crate::logln!("rates: rebuilt {} buckets from {replayed} contract-2 reports", snapshot.book.buckets.len());
    Ok(snapshot)
}

fn pairs_value(map: &BTreeMap<(String, String), f64>) -> Value {
    Value::Arr(map.iter().map(|((first, second), sum)| Value::Arr(vec![str_value(first), str_value(second), Value::Float(*sum)])).collect())
}

fn read_pairs(value: &Value) -> BTreeMap<(String, String), f64> {
    let mut out = BTreeMap::new();
    for row in value.as_arr().cloned().unwrap_or_default() {
        let Some(parts) = row.as_arr() else { continue };
        if let (Some(first), Some(second), Some(sum)) = (parts.first().and_then(|part| part.as_str()), parts.get(1).and_then(|part| part.as_str()), parts.get(2).and_then(|part| part.as_f64())) {
            out.insert((first.to_string(), second.to_string()), sum);
        }
    }
    out
}

fn bucket_value(key: &YieldKey, bucket: &Bucket) -> Value {
    object(vec![
        ("method", str_value(&key.method)),
        ("style", str_value(&key.style)),
        ("site", str_value(&key.site)),
        ("tool", str_value(&key.tool)),
        ("band", Value::Int(key.band)),
        ("runs", Value::Int(bucket.runs as i64)),
        ("weight", Value::Float(bucket.weight)),
        ("work_ticks", Value::Float(bucket.work_ticks)),
        ("pause_ticks", Value::Float(bucket.pause_ticks)),
        ("flows", pairs_value(&bucket.flows)),
        ("units", pairs_value(&bucket.units)),
        ("rate_sum", Value::Float(bucket.rate_sum)),
        ("rate_squares", Value::Float(bucket.rate_squares)),
        ("finished", Value::Float(bucket.finished)),
        ("failed", Value::Float(bucket.failed)),
        ("rejected", Value::Int(bucket.rejected as i64)),
        ("last_clock", Value::Int(bucket.last_clock)),
    ])
}

fn read_bucket(value: &Value) -> Option<(YieldKey, Bucket)> {
    let text = |key: &str| value.get(key).as_str().map(str::to_string);
    let float = |key: &str| value.get(key).as_f64().unwrap_or(0.0);
    let key = YieldKey { method: text("method")?, style: text("style")?, site: text("site")?, tool: text("tool")?, band: value.get("band").as_i64()? };
    let bucket = Bucket {
        runs: value.get("runs").as_i64().unwrap_or(0).max(0) as u64,
        weight: float("weight"),
        work_ticks: float("work_ticks"),
        pause_ticks: float("pause_ticks"),
        flows: read_pairs(value.get("flows")),
        units: read_pairs(value.get("units")),
        rate_sum: float("rate_sum"),
        rate_squares: float("rate_squares"),
        finished: float("finished"),
        failed: float("failed"),
        rejected: value.get("rejected").as_i64().unwrap_or(0).max(0) as u64,
        last_clock: value.get("last_clock").as_i64().unwrap_or(0),
    };
    Some((key, bucket))
}

pub(super) fn snapshot_to_value(snapshot: &Snapshot) -> Value {
    let (trade_items, trade_last) = snapshot.trades.to_value();
    object(vec![
        ("schema", Value::Int(SNAPSHOT_SCHEMA)),
        ("buckets", Value::Arr(snapshot.book.buckets.iter().map(|(key, bucket)| bucket_value(key, bucket)).collect())),
        ("trades", trade_items),
        ("trade_last", trade_last),
    ])
}

/// A schema-4 file, `None` for anything else (rebuilt from the store).
pub(crate) fn snapshot_from_json(text: &str) -> Option<Snapshot> {
    let value = lifescape_json::parse(text).ok()?;
    if value.get("schema").as_i64() != Some(SNAPSHOT_SCHEMA) {
        return None;
    }
    let mut book = YieldBook::default();
    for row in value.get("buckets").as_arr().cloned().unwrap_or_default() {
        if let Some((key, bucket)) = read_bucket(&row) {
            book.buckets.insert(key, bucket);
        }
    }
    Some(Snapshot { book, trades: trading::TradeBook::from_value(value.get("trades"), value.get("trade_last")) })
}

/// Writes the live snapshot atomically (tmp + rename); returns the bytes
/// written. Loud error, never a silent drop: the flusher logs and retries.
pub(crate) fn write_snapshot_file() -> Result<usize, String> {
    let shared = global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let value = snapshot_to_value(&shared);
    let path = snapshot_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("rates snapshot dir: {error}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = lifescape_json::stringify(&value);
    std::fs::write(&tmp, &text).map_err(|error| format!("rates snapshot write: {error}"))?;
    std::fs::rename(&tmp, &path).map_err(|error| format!("rates snapshot rename: {error}"))?;
    Ok(text.len())
}
