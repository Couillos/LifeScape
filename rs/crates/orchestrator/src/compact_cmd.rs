//! `orchestrator store-compact` (2026-11-23): rewrites `run/lifescape.db`
//! without its free pages (`VACUUM`) and empties the WAL. Migration 13
//! drops indexes and rebuilds tables, which frees pages inside the file
//! but never returns them to the disk: this is the explicit, operator-run
//! step that does (a `VACUUM` cannot run inside a migration, and the
//! daemon never runs one on its own).
//!
//! Offline: refuses while the daemon runs. Needs free disk space of about
//! the size of the database (SQLite writes a full copy, then swaps it in).
//! Nothing is lost: `VACUUM` rewrites every row as it is.

#![forbid(unsafe_code)]

fn size_of(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// CLI entry; returns the process exit code.
pub fn store_compact_cli(_args: &[String]) -> i32 {
    let daemon_pid = std::fs::read_to_string(crate::paths::pid_path()).ok().and_then(|raw| raw.trim().parse::<u32>().ok());
    if daemon_pid.is_some_and(crate::worker_owner::pid_alive) {
        eprintln!("store-compact: the daemon runs (pid {}); stop it first (shutdown)", daemon_pid.unwrap_or(0));
        return 1;
    }
    let path = crate::market_service::db_path();
    if !path.exists() {
        eprintln!("store-compact: no database at {}", path.display());
        return 1;
    }
    let before = size_of(&path);
    // Opening applies any pending migration (idempotent otherwise).
    let store = match lifescape_store::Store::open(&path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("store-compact: store open: {error}");
            return 1;
        }
    };
    if let Err(error) = store.compact() {
        eprintln!("store-compact: {error}");
        return 1;
    }
    drop(store);
    println!("store-compact: {} -> {} ({})", megabytes(before), megabytes(size_of(&path)), path.display());
    0
}
