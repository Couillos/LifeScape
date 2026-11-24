#![forbid(unsafe_code)]
//! Retention of what piles up on disk (plan d'architecture, decision 3,
//! 2026-11-24): `run/failures/` dossiers past 7 days and cancelled orders
//! (with their `order_amends`) past 14 days. One function does the work
//! (`run_once`), called by the hourly `retention` daemon thread and by
//! `orchestrator retention [--check]`; `--check` counts, deletes nothing.
//!
//! Kept for good: open, matched and settled orders, deals, executions.
//! Why 14 days: the longest window that reads cancelled orders is 7 days
//! (the economy page's order outcomes; the seller's learned floor reads
//! 24 h, `market_own::LEARNING_WINDOW_SECS`), see `lifescape_store::retention`.

use std::path::Path;
use std::time::Duration;

use lifescape_store::{ExpiredOrders, Store};

/// Dossiers older than this are removed (file mtime).
pub const FAILURES_KEEP_SECS: i64 = 7 * 86_400;
/// Cancelled orders last updated longer ago than this are removed.
pub const CANCELLED_KEEP_SECS: i64 = 14 * 86_400;
/// Orders removed per transaction (short writer lock, carnet stays live).
pub const BATCH_ORDERS: i64 = 2_000;
/// Pause between two batches.
const BATCH_PAUSE: Duration = Duration::from_millis(250);
/// First run after the start, then every `EVERY`.
const FIRST_AFTER: Duration = Duration::from_secs(600);
const EVERY: Duration = Duration::from_secs(3_600);

/// What a run counted (`check`) or removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub orders: ExpiredOrders,
    pub dossiers: usize,
    pub dossier_bytes: u64,
}

/// `.json` files of `dir` last modified before `cutoff` (unix seconds),
/// removed unless `check`. Returns (files, bytes). A missing directory is
/// nothing to do; an unreadable entry is skipped.
pub fn purge_dossiers(dir: &Path, cutoff: i64, check: bool) -> Result<(usize, u64), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let (mut files, mut bytes) = (0usize, 0u64);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Some(mtime) = meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map(|span| span.as_secs() as i64) else { continue };
        if mtime >= cutoff {
            continue;
        }
        if !check && std::fs::remove_file(&path).is_err() {
            continue;
        }
        files += 1;
        bytes += meta.len();
    }
    Ok((files, bytes))
}

/// One retention pass at `now` (unix seconds). `check`: counts only.
pub fn run_once(store: &Store, failures_dir: &Path, now: i64, check: bool) -> Result<Report, String> {
    let orders_cutoff = now - CANCELLED_KEEP_SECS;
    let mut report = Report::default();
    if check {
        report.orders = store.count_expired_orders(orders_cutoff).map_err(|error| error.to_string())?;
    } else {
        loop {
            let batch = store.purge_expired_orders(orders_cutoff, BATCH_ORDERS).map_err(|error| error.to_string())?;
            if batch.orders == 0 {
                break;
            }
            report.orders.orders += batch.orders;
            report.orders.amends += batch.amends;
            std::thread::sleep(BATCH_PAUSE);
        }
    }
    let (dossiers, bytes) = purge_dossiers(failures_dir, now - FAILURES_KEEP_SECS, check)?;
    report.dossiers = dossiers;
    report.dossier_bytes = bytes;
    Ok(report)
}

/// The `retention` daemon thread: a pass ten minutes after the start,
/// then every hour; a line only when something was removed.
pub fn start() {
    let spawned = std::thread::Builder::new().name("retention".to_string()).spawn(|| {
        // Gauge period, not a game wait.
        std::thread::sleep(FIRST_AFTER);
        loop {
            match Store::open(&crate::paths::store_db()) {
                Ok(store) => match run_once(&store, &crate::paths::run_dir().join("failures"), crate::jobs::now_secs() as i64, false) {
                    Ok(report) if report.orders.orders > 0 || report.dossiers > 0 => crate::logln!(
                        "retention: purged {} cancelled orders (+{} amends) and {} failure dossiers ({:.1} MB)",
                        report.orders.orders,
                        report.orders.amends,
                        report.dossiers,
                        report.dossier_bytes as f64 / 1_048_576.0
                    ),
                    Ok(_) => {}
                    Err(error) => crate::logln!("retention: pass failed: {error}"),
                },
                Err(error) => crate::logln!("retention: store open failed: {error}"),
            }
            std::thread::sleep(EVERY);
        }
    });
    if let Err(error) = spawned {
        eprintln!("retention thread spawn failed ({error}): no purge");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_age(path: &Path, age_secs: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).expect("open");
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(age_secs)).expect("mtime");
    }

    /// Dossiers past 7 days go, newer ones and other files stay; `check`
    /// counts the same files and removes none.
    #[test]
    fn old_dossiers_are_purged_and_check_only_counts() {
        let dir = std::env::temp_dir().join(format!("lifescape-retention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, days) in [("a-1.json", 9u64), ("a-2.json", 8), ("a-3.json", 6), ("a-4.json", 0), ("note.txt", 30)] {
            let path = dir.join(name);
            std::fs::write(&path, vec![b'x'; 100]).unwrap();
            set_age(&path, days * 86_400 + 60);
        }
        std::fs::create_dir_all(dir.join("sub.json")).unwrap();
        let now = crate::jobs::now_secs() as i64;
        let cutoff = now - FAILURES_KEEP_SECS;
        assert_eq!(purge_dossiers(&dir, cutoff, true).unwrap(), (2, 200));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 6, "check removes nothing");
        assert_eq!(purge_dossiers(&dir, cutoff, false).unwrap(), (2, 200));
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, ["a-3.json", "a-4.json", "note.txt", "sub.json"]);
        assert_eq!(purge_dossiers(&dir.join("missing"), cutoff, false).unwrap(), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole pass on an in-memory store: counts under `check`, then
    /// the same figures removed; recent cancelled orders stay.
    #[test]
    fn a_pass_counts_then_purges_the_same_rows() {
        use lifescape_store::{NewOrder, OrderSide};
        let store = Store::open_in_memory().unwrap();
        store.upsert_item(&lifescape_store::Item { item_id: 314, name: "Test item".to_string(), shop_price: 2, alch_value: 1 }).unwrap();
        let dir = std::env::temp_dir().join(format!("lifescape-retention-pass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = 1_800_000_000i64;
        let place = |clock: i64| {
            let id = store.place_order(&NewOrder { account: "t01".to_string(), side: OrderSide::Sell, item_id: 314, quantity: 5, price: 9, dormant: false, clock, replaces: None, motive: "surplus".to_string(), ceiling: None, ask_limit: None }).unwrap();
            store.cancel_order_because(id, clock + 10, "horizon").unwrap();
        };
        for _ in 0..5 {
            place(now - 15 * 86_400);
        }
        for _ in 0..3 {
            place(now - 13 * 86_400);
        }
        let checked = run_once(&store, &dir, now, true).unwrap();
        assert_eq!(checked.orders.orders, 5);
        let done = run_once(&store, &dir, now, false).unwrap();
        assert_eq!(done.orders, checked.orders);
        assert_eq!(run_once(&store, &dir, now, true).unwrap().orders.orders, 0);
    }
}
