//! `orchestrator retention [--check]` (plan d'architecture, decision 3):
//! the operator's view of the retention the daemon applies every hour
//! (`lifescape_orch_daemon::retention`): failure dossiers older than 7 days
//! and cancelled orders (with their `order_amends`) older than 14 days.
//!
//! `--check` is a dry run: it counts what a pass would remove and deletes
//! nothing; it may run while the daemon runs. Without `--check` the purge
//! refuses while the daemon runs (the daemon purges by itself every hour:
//! stop it first to purge by hand, for example before a store compaction).

#![forbid(unsafe_code)]

use crate::retention::{run_once, CANCELLED_KEEP_SECS, FAILURES_KEEP_SECS};

/// CLI entry; returns the process exit code.
pub fn retention_cli(args: &[String]) -> i32 {
    let check = args.iter().any(|arg| arg == "--check");
    if let Some(unknown) = args.iter().find(|arg| arg.as_str() != "--check") {
        eprintln!("retention: unknown argument {unknown:?} (usage: orchestrator retention [--check])");
        return 2;
    }
    if !check {
        let daemon_pid = std::fs::read_to_string(crate::paths::pid_path()).ok().and_then(|raw| raw.trim().parse::<u32>().ok());
        if daemon_pid.is_some_and(crate::worker_owner::pid_alive) {
            eprintln!("retention: the daemon runs (pid {}) and purges by itself every hour; use --check to count, or stop it first", daemon_pid.unwrap_or(0));
            return 1;
        }
    }
    let store = match lifescape_store::Store::open(&crate::market_service::db_path()) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("retention: store open: {error}");
            return 1;
        }
    };
    let now = crate::jobs::now_secs() as i64;
    match run_once(&store, &crate::paths::run_dir().join("failures"), now, check) {
        Ok(report) => {
            let verb = if check { "would remove" } else { "removed" };
            println!(
                "retention ({}): {verb} {} cancelled orders older than {} days (+{} order_amends), {} failure dossiers older than {} days ({:.1} MB)",
                if check { "dry run" } else { "done" },
                report.orders.orders,
                CANCELLED_KEEP_SECS / 86_400,
                report.orders.amends,
                report.dossiers,
                FAILURES_KEEP_SECS / 86_400,
                report.dossier_bytes as f64 / 1_048_576.0
            );
            0
        }
        Err(error) => {
            eprintln!("retention: {error}");
            1
        }
    }
}
