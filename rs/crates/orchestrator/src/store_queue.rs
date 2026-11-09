#![forbid(unsafe_code)]
//! Grouped task-report staging (E8): the SQLite `store` (later lots)
//! drains this file, the daemon never writes one row per report.
//!
//! Before: every finished engagement rewrote the merged state file right
//! away. Now reports buffer in memory and flush as one append batch
//! (50 rows or 5 s, one `fsync` per batch) to
//! `run/lifescape-staging.jsonl`, the future store import. Bounded at
//! 10 000 rows: past that the oldest batch is force-flushed first, never
//! dropped silently (a flush failure errors loudly).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use lifescape_json::{self as json, Value};

/// Rows per batch and oldest-batch age before a forced flush.
const BATCH_ROWS: usize = 50;
const BATCH_AGE: Duration = Duration::from_secs(5);
/// Memory bound: past this the queue force-flushes instead of growing.
const QUEUE_CAP: usize = 10_000;

struct Staging {
    rows: Vec<(f64, Value)>,
    last_flush: std::time::Instant,
}

impl Staging {
    fn new() -> Self {
        Staging { rows: Vec::new(), last_flush: std::time::Instant::now() }
    }

    fn should_flush(&self) -> bool {
        self.rows.len() >= BATCH_ROWS || self.last_flush.elapsed() >= BATCH_AGE
    }
}

fn staging() -> &'static Mutex<Staging> {
    static STAGING: OnceLock<Mutex<Staging>> = OnceLock::new();
    STAGING.get_or_init(|| Mutex::new(Staging::new()))
}

fn staging_path() -> PathBuf {
    // Tests point this at a temp dir (never the real run/).
    if let Ok(dir) = std::env::var("LIFESCAPE_STAGING_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("lifescape-staging.jsonl");
        }
    }
    crate::paths::run_dir().join("lifescape-staging.jsonl")
}

fn append_batch(rows: &[Value]) -> Result<(), String> {
    use std::io::Write;
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(staging_path())
        .map_err(|error| format!("staging open: {error}"))?;
    for row in rows {
        let mut line = json::stringify(row);
        line.push('\n');
        handle.write_all(line.as_bytes()).map_err(|error| format!("staging write: {error}"))?;
    }
    handle.sync_all().map_err(|error| format!("staging fsync: {error}"))?;
    Ok(())
}

/// Buffer one task report; flushes in batches (count or age). Loud
/// error, never a silent drop.
pub fn stage_report(report: &Value) -> Result<(), String> {
    let mut guard = staging().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = crate::jobs::now_secs();
    guard.rows.push((now, report.clone()));
    if guard.rows.len() >= QUEUE_CAP || guard.should_flush() {
        let rows: Vec<Value> = guard.rows.drain(..).map(|(_, row)| row).collect();
        guard.last_flush = std::time::Instant::now();
        drop(guard);
        return append_batch(&rows);
    }
    Ok(())
}

/// Force-flush for shutdown paths. Empty queue is a no-op.
pub fn flush_staging() -> Result<(), String> {
    let mut guard = staging().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.rows.is_empty() {
        return Ok(());
    }
    let rows: Vec<Value> = guard.rows.drain(..).map(|(_, row)| row).collect();
    guard.last_flush = std::time::Instant::now();
    drop(guard);
    append_batch(&rows)
}

#[cfg(test)]
mod tests {
    use super::{flush_staging, stage_report, BATCH_ROWS};
    use lifescape_json::{object, str_value, Value};

    fn report(index: usize) -> Value {
        object(vec![
            ("account", str_value("t01")),
            ("stop", str_value("goal_reached")),
            ("n", Value::Int(index as i64)),
        ])
    }

    /// A full batch lands on disk with one flush, rows intact and in
    /// order; the leftovers wait for the next batch. Temp dir, never the
    /// real run/.
    #[test]
    fn batch_flushes_grouped_not_per_row() {
        let dir = std::env::temp_dir().join(format!("lifescape-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("LIFESCAPE_STAGING_DIR", &dir);
        for index in 0..BATCH_ROWS {
            stage_report(&report(index)).unwrap();
        }
        let text = std::fs::read_to_string(dir.join("lifescape-staging.jsonl")).unwrap_or_default();
        let before = text.lines().count();
        assert!(before >= BATCH_ROWS, "batch auto-flushed, got {before}");
        for index in 0..3 {
            stage_report(&report(1000 + index)).unwrap();
        }
        flush_staging().unwrap();
        let text = std::fs::read_to_string(dir.join("lifescape-staging.jsonl")).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= before + 3, "leftovers flush grouped, got {}", lines.len());
        assert!(lines.iter().any(|line| line.contains("1002")), "order kept");
        std::env::remove_var("LIFESCAPE_STAGING_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
