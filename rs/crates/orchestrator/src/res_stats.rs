#![forbid(unsafe_code)]
//! Daemon resource counters (first attribution chantier): cumulative counts
//! plus per-second rates since daemon start, read by `res-stats`.
//!
//! Recorded here: accepted daemon-socket connections (`main.rs`), worker
//! push frames and their bytes (`daemon.rs::ingest_push`), state files
//! written (`daemon.rs::write_state`). Plan submits and polls ride along
//! from the pathfinding service gauge (`plan_service::stats_json`), so one
//! command shows pushes, bytes, plan requests, polls, files and
//! connections. Rates are totals divided by uptime: honest averages, not
//! windowed peaks.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use lifescape_json::{Value, object};

struct Counters {
    started: std::time::Instant,
    connections: AtomicU64,
    pushes: AtomicU64,
    push_bytes: AtomicU64,
    files_written: AtomicU64,
}

fn counters() -> &'static Counters {
    static COUNTERS: OnceLock<Counters> = OnceLock::new();
    COUNTERS.get_or_init(|| Counters {
        started: std::time::Instant::now(),
        connections: AtomicU64::new(0),
        pushes: AtomicU64::new(0),
        push_bytes: AtomicU64::new(0),
        files_written: AtomicU64::new(0),
    })
}

/// One accepted connection on the daemon socket (CLI, worker push,
/// session command: the churn this counts is the point).
pub fn record_connection() {
    counters().connections.fetch_add(1, Ordering::Relaxed);
}

/// One worker push frame received, with its approximate on-wire bytes.
pub fn record_push(bytes: u64) {
    counters().pushes.fetch_add(1, Ordering::Relaxed);
    counters().push_bytes.fetch_add(bytes, Ordering::Relaxed);
}

/// One state file written by the daemon.
pub fn record_file() {
    counters().files_written.fetch_add(1, Ordering::Relaxed);
}

fn per_second(total: u64, uptime_secs: f64) -> f64 {
    if uptime_secs > 0.0 { total as f64 / uptime_secs } else { 0.0 }
}

/// Operator gauge: cumulative counts with per-second averages, plus the
/// pathfinding plan/poll counts when the service is up. Missing service
/// reads zero plans, never stale numbers.
pub fn snapshot_json() -> Value {
    let counts = counters();
    let uptime_secs = counts.started.elapsed().as_secs_f64();
    let connections = counts.connections.load(Ordering::Relaxed);
    let pushes = counts.pushes.load(Ordering::Relaxed);
    let push_bytes = counts.push_bytes.load(Ordering::Relaxed);
    let files = counts.files_written.load(Ordering::Relaxed);
    let plan = crate::plan_service::stats_json();
    let plan_num = |key: &str| plan.as_ref().and_then(|stats| stats.get(key).as_i64()).unwrap_or(0);
    object(vec![
        ("ok", Value::Bool(true)),
        ("uptime_secs", Value::Float(uptime_secs)),
        ("connections", Value::Int(connections as i64)),
        ("connections_per_s", Value::Float(per_second(connections, uptime_secs))),
        ("pushes", Value::Int(pushes as i64)),
        ("pushes_per_s", Value::Float(per_second(pushes, uptime_secs))),
        ("push_bytes", Value::Int(push_bytes.min(i64::MAX as u64) as i64)),
        ("push_bytes_per_s", Value::Float(per_second(push_bytes, uptime_secs))),
        ("plan_submits", Value::Int(plan_num("submits"))),
        ("plan_submits_per_s", Value::Float(per_second(plan_num("submits") as u64, uptime_secs))),
        ("plan_polls", Value::Int(plan_num("polls"))),
        ("plan_polls_per_s", Value::Float(per_second(plan_num("polls") as u64, uptime_secs))),
        ("files_written", Value::Int(files as i64)),
        ("files_per_s", Value::Float(per_second(files, uptime_secs))),
    ])
}

#[cfg(test)]
mod tests {
    use super::{per_second, snapshot_json};

    /// Averages divide by uptime; zero uptime never divides by zero.
    #[test]
    fn rates_average_over_uptime() {
        assert_eq!(per_second(0, 0.0), 0.0);
        assert!((per_second(100, 50.0) - 2.0).abs() < 1e-9);
    }

    /// The gauge always answers with every counter present, service or not.
    #[test]
    fn snapshot_carries_every_counter() {
        let snapshot = snapshot_json();
        for key in [
            "connections",
            "connections_per_s",
            "pushes",
            "pushes_per_s",
            "push_bytes",
            "push_bytes_per_s",
            "plan_submits",
            "plan_submits_per_s",
            "plan_polls",
            "plan_polls_per_s",
            "files_written",
            "files_per_s",
        ] {
            assert!(!snapshot.get(key).is_null(), "res-stats needs {key}");
        }
    }
}
