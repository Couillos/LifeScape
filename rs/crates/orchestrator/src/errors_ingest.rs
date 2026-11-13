#![forbid(unsafe_code)]
//! Error collector (errors page): one daemon thread feeds the
//! `error_events` table of the store.
//!
//! Pushed sources go through a bounded channel, never blocking the
//! caller: script error events and failed/blocked task reports (from
//! `ingest_push`), brain lines (from `logln!`). Pulled sources are
//! rescanned every 30 s: nav dossiers (`run/failures/*.json`, by mtime)
//! and failed deals (`deals` table). Pulled rows carry a unique
//! `source_ref`, so rescans and restarts never duplicate. Rows older than
//! the retention are purged hourly. A full channel or a store failure is
//! logged, never silent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use lifescape_json::{self as json, Value};
use lifescape_store::{ErrorRecord, Store, TaskOutcome};

use crate::daemon::Orchestrator;
use crate::errors_model;
use crate::jobs::Job;

/// Rows kept (and backfilled) this long: nav dossiers alone run at
/// ~500–1400 rows/h (~650 B each), 72 h keeps the table near 50–100 MB.
pub const RETENTION_SECS: i64 = 72 * 3_600;
const CHANNEL_CAP: usize = 8_192;
const BATCH_ROWS: usize = 200;
const BATCH_AGE: Duration = Duration::from_secs(2);
const SCAN_EVERY: Duration = Duration::from_secs(30);
const PURGE_EVERY: Duration = Duration::from_secs(3_600);
/// Pending intake kept across store failures before dropping (logged).
const PENDING_CAP: usize = 20_000;
/// A dossier this fresh takes its task from the job's current run.
const FRESH_DOSSIER_SECS: i64 = 90;

enum Intake {
    Event { account: String, frame: Value, clock: i64 },
    Report { account: String, report: Value, position: Option<(i64, i64, i64)>, clock: i64 },
    Brain { line: String, clock: i64 },
}

static SENDER: OnceLock<SyncSender<Intake>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn now_clock() -> i64 {
    crate::jobs::now_secs() as i64
}

fn send(intake: Intake) {
    let Some(sender) = SENDER.get() else {
        return;
    };
    match sender.try_send(intake) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            // Counted here, reported by the collector (logging from inside
            // a `logln!` hook would recurse).
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A pushed worker event; only `script_error` events become rows.
pub fn observe_event(account: &str, frame: &Value) {
    if frame.get("kind").as_str() != Some("script_error") {
        return;
    }
    send(Intake::Event { account: account.to_string(), frame: frame.clone(), clock: now_clock() });
}

/// A pushed task report; only failed/blocked stops become rows. The
/// position is the failure's own (`last_error.at` when it is the same
/// message), else the live one. Call before locking the job.
pub fn observe_report(account: &str, report: &Value, job: &Job) {
    let stop = report.get("stop").as_str().unwrap_or("");
    if stop != "failed" && stop != "blocked" {
        return;
    }
    let position = job.inner.try_lock().ok().and_then(|inner| {
        let message = report.get("error").get("message").as_str().unwrap_or("");
        let from_error = inner
            .last_error
            .as_ref()
            .filter(|error| !message.is_empty() && error.get("message").as_str() == Some(message))
            .and_then(|error| errors_model::position_of(error.get("at")));
        from_error.or_else(|| inner.live.as_ref().and_then(|live| errors_model::position_of(live.get("position"))))
    });
    send(Intake::Report { account: account.to_string(), report: report.clone(), position, clock: now_clock() });
}

/// Hook of every daemon log line (`logln!`): brain abandons, stuck
/// graphs and goals, bans. Cheap prefix test first; a no-op until the
/// collector runs (workers and CLI never start it).
pub fn observe_log_line(line: &str) {
    if !line.starts_with("brain ") || SENDER.get().is_none() {
        return;
    }
    let wanted = line.contains(" -> abandoned (")
        || line.contains(": graph stuck")
        || line.contains(": goal stuck")
        || line.contains(" banned ")
        || line.contains(": revive cap reached");
    if wanted {
        send(Intake::Brain { line: line.to_string(), clock: now_clock() });
    }
}

/// Start the collector thread once (daemon only).
pub fn start(orch: &Arc<Orchestrator>) {
    let (sender, receiver) = sync_channel::<Intake>(CHANNEL_CAP);
    if SENDER.set(sender).is_err() {
        return;
    }
    let weak = Arc::downgrade(orch);
    std::thread::Builder::new()
        .name("errors-collector".to_string())
        .spawn(move || collect(receiver, weak))
        .expect("errors collector spawns");
}

fn db_path() -> PathBuf {
    crate::paths::run_dir().join("lifescape.db")
}

fn failures_dir() -> PathBuf {
    crate::paths::run_dir().join("failures")
}

fn open_store(slot: &mut Option<Store>) -> Result<&Store, String> {
    if slot.is_none() {
        *slot = Some(Store::open(&db_path()).map_err(|error| format!("errors store open: {error}"))?);
    }
    slot.as_ref().ok_or_else(|| "errors store missing".to_string())
}

fn collect(receiver: Receiver<Intake>, orch: Weak<Orchestrator>) {
    let mut slot: Option<Store> = None;
    let mut pending: Vec<Intake> = Vec::new();
    let mut last_flush = Instant::now();
    let mut last_scan: Option<Instant> = None;
    let mut last_purge: Option<Instant> = None;
    let mut dossier_cursor: Option<i64> = None;
    loop {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(intake) => pending.push(intake),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while pending.len() < BATCH_ROWS * 4 {
            match receiver.try_recv() {
                Ok(intake) => pending.push(intake),
                Err(_) => break,
            }
        }
        let dropped = DROPPED.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            crate::logln!("errors: channel full, {dropped} error rows dropped");
        }
        if !pending.is_empty() && (pending.len() >= BATCH_ROWS || last_flush.elapsed() >= BATCH_AGE) {
            last_flush = Instant::now();
            let result = open_store(&mut slot).and_then(|store| flush(store, &pending, &orch));
            match result {
                Ok(()) => pending.clear(),
                Err(error) => {
                    crate::logln!("errors: flush of {} rows failed: {error}", pending.len());
                    slot = None;
                    if pending.len() > PENDING_CAP {
                        let excess = pending.len() - PENDING_CAP;
                        pending.drain(..excess);
                        crate::logln!("errors: {excess} oldest pending rows dropped (store down)");
                    }
                }
            }
        }
        if last_scan.is_none_or(|at| at.elapsed() >= SCAN_EVERY) {
            last_scan = Some(Instant::now());
            let result = open_store(&mut slot).and_then(|store| {
                let deals = sync_deals(store)?;
                let dossiers = scan_dossiers(store, &failures_dir(), &mut dossier_cursor, &orch)?;
                Ok((deals, dossiers))
            });
            match result {
                Ok((deals, dossiers)) if deals + dossiers > 0 => {
                    crate::logln!("errors: pulled {dossiers} nav dossiers, {deals} failed deals");
                }
                Ok(_) => {}
                Err(error) => {
                    crate::logln!("errors: pull failed: {error}");
                    slot = None;
                }
            }
        }
        if last_purge.is_none_or(|at| at.elapsed() >= PURGE_EVERY) {
            last_purge = Some(Instant::now());
            if let Ok(store) = open_store(&mut slot) {
                match store.purge_errors_before(now_clock() - RETENTION_SECS) {
                    Ok(removed) if removed > 0 => crate::logln!("errors: purged {removed} rows past retention"),
                    Ok(_) => {}
                    Err(error) => crate::logln!("errors: purge failed: {error}"),
                }
            }
        }
    }
}

/// Live context of an account: (position, script, args) from the job.
fn live_context(orch: &Weak<Orchestrator>, account: &str) -> (Option<(i64, i64, i64)>, String, String) {
    let Some(orch) = orch.upgrade() else {
        return (None, String::new(), String::new());
    };
    let job = orch.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).cloned();
    let Some(job) = job else {
        return (None, String::new(), String::new());
    };
    let Ok(inner) = job.inner.try_lock() else {
        return (None, String::new(), String::new());
    };
    let position = inner.live.as_ref().and_then(|live| errors_model::position_of(live.get("position")));
    let script = inner.script.clone().unwrap_or_default();
    let args: Vec<String> = inner.script_args.iter().filter_map(|arg| arg.as_str().map(str::to_string)).collect();
    (position, script, args.join(" "))
}

/// Writes one batch in arrival order: events and brain rows insert,
/// reports fold onto their event row or insert their own.
fn flush(store: &Store, intakes: &[Intake], orch: &Weak<Orchestrator>) -> Result<(), String> {
    let zone_of = &errors_model::map_zone;
    let mut batch: Vec<ErrorRecord> = Vec::new();
    for intake in intakes {
        match intake {
            Intake::Event { account, frame, clock } => {
                if let Some(row) = errors_model::from_event(account, frame, *clock, zone_of) {
                    batch.push(row);
                }
            }
            Intake::Brain { line, clock } => {
                if let Some(mut row) = errors_model::from_brain_line(line, *clock) {
                    let (position, script, args) = live_context(orch, &row.account);
                    if row.script.is_empty() {
                        row.script = script;
                        row.args = args;
                    }
                    row.position = position;
                    row.zone = position.map(|(pos_x, pos_z, _)| zone_of(pos_x, pos_z)).unwrap_or_default();
                    batch.push(row);
                }
            }
            Intake::Report { account, report, position, clock } => {
                let Some((outcome, row)) = errors_model::from_report(account, report, *position, *clock, zone_of) else {
                    continue;
                };
                store.record_errors(&batch).map_err(|error| error.to_string())?;
                batch.clear();
                if !fold(store, &outcome)? {
                    batch.push(row);
                }
            }
        }
    }
    store.record_errors(&batch).map_err(|error| error.to_string())?;
    Ok(())
}

fn fold(store: &Store, outcome: &TaskOutcome) -> Result<bool, String> {
    store.attach_task_outcome(outcome).map_err(|error| error.to_string())
}

/// Failed deals since the newest deal row (rescan-safe by deal id).
fn sync_deals(store: &Store) -> Result<usize, String> {
    let since = match store.latest_error_clock("deal").map_err(|error| error.to_string())? {
        Some(clock) => clock,
        None => now_clock() - RETENTION_SECS,
    };
    let deals = store.failed_deals_since(since).map_err(|error| error.to_string())?;
    let rows: Vec<ErrorRecord> = deals.iter().map(|deal| errors_model::from_deal(deal, &errors_model::map_zone)).collect();
    store.record_errors(&rows).map_err(|error| error.to_string())
}

fn mtime_of(entry: &std::fs::DirEntry) -> Option<i64> {
    let modified = entry.metadata().ok()?.modified().ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Dossiers written at or after the cursor (first pass: the newest nav
/// row, else the retention start), oldest first, in batches of 500.
pub(crate) fn scan_dossiers(
    store: &Store,
    dir: &Path,
    cursor: &mut Option<i64>,
    orch: &Weak<Orchestrator>,
) -> Result<usize, String> {
    let start = match *cursor {
        Some(clock) => clock,
        None => store
            .latest_error_clock("nav")
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| now_clock() - RETENTION_SECS),
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("failures dir {}: {error}", dir.display())),
    };
    let mut fresh: Vec<(i64, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") {
            continue;
        }
        if let Some(mtime) = mtime_of(&entry) {
            // One second of slack: a file written in the cursor's second
            // may have been missed; the unique ref drops the repeat.
            if mtime >= start - 1 {
                fresh.push((mtime, name));
            }
        }
    }
    fresh.sort();
    let now = now_clock();
    let mut inserted = 0;
    let mut newest = start;
    for chunk in fresh.chunks(500) {
        let mut rows = Vec::with_capacity(chunk.len());
        for (mtime, name) in chunk {
            newest = newest.max(*mtime);
            let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
                continue;
            };
            let Ok(dossier) = json::parse(&text) else {
                continue;
            };
            let Some(mut row) = errors_model::from_dossier(name, *mtime, &dossier, &errors_model::map_zone) else {
                continue;
            };
            if now - mtime <= FRESH_DOSSIER_SECS {
                let (_, script, args) = live_context(orch, &row.account);
                row.node_id = errors_model::node_of_args(&args.split(' ').map(str::to_string).collect::<Vec<_>>());
                row.script = script;
                row.args = args;
            }
            rows.push(row);
        }
        inserted += store.record_errors(&rows).map_err(|error| error.to_string())?;
    }
    *cursor = Some(newest);
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dossiers land once whatever the rescans; non-JSON files and
    /// unparsable dossiers are skipped. Temp dir, never the real run/.
    #[test]
    fn dossier_scan_is_rescan_safe() {
        let dir = std::env::temp_dir().join(format!("lifescape-errors-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dossier = r#"{"account":"t01","from":{"level":0,"x":10,"z":20},"error":{"kind":"NoRoute","observed":"travel: no route"}}"#;
        std::fs::write(dir.join("t01-5.json"), dossier).unwrap();
        std::fs::write(dir.join("t01-6.json"), "{not json").unwrap();
        std::fs::write(dir.join("notes.txt"), dossier).unwrap();
        let store = Store::open_in_memory().unwrap();
        let orch: Weak<Orchestrator> = Weak::new();
        let mut cursor = None;
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 1);
        assert!(cursor.is_some());
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0, "rescan adds nothing");
        let mut fresh_cursor = None;
        assert_eq!(scan_dossiers(&store, &dir, &mut fresh_cursor, &orch).unwrap(), 0, "restart adds nothing");
        let rows = store.errors_matching(&lifescape_store::ErrorFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dossier, "t01-5.json");
        let missing = dir.join("absent");
        assert_eq!(scan_dossiers(&store, &missing, &mut None, &orch).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed report folds onto its script error row (one failure, one
    /// row); a blocked report with no event records its own row.
    #[test]
    fn reports_fold_onto_events() {
        let store = Store::open_in_memory().unwrap();
        let frame = json::parse(
            r#"{"kind":"script_error","script":"fishing","args":["--node","n4"],"message":"failed: fish failed at (0, 1, 2)","at":[0,1,2]}"#,
        )
        .unwrap();
        let failed = json::parse(
            r#"{"account":"t01","activity":"fishing","node_id":"n4","stop":"failed","error":{"type":"Failed","message":"failed: fish failed at (0, 1, 2)"}}"#,
        )
        .unwrap();
        let blocked = json::parse(r#"{"account":"t01","activity":"smith","node_id":"n9","stop":"blocked","missing":["hammer"]}"#).unwrap();
        let intakes = vec![
            Intake::Event { account: "t01".to_string(), frame, clock: 100 },
            Intake::Report { account: "t01".to_string(), report: failed, position: None, clock: 101 },
            Intake::Report { account: "t01".to_string(), report: blocked, position: Some((5, 6, 0)), clock: 102 },
            Intake::Brain { line: "brain t01: graph stuck, nothing left to run; abandoned: n9=x".to_string(), clock: 103 },
        ];
        flush(&store, &intakes, &Weak::new()).unwrap();
        let rows = store.errors_matching(&lifescape_store::ErrorFilter::default()).unwrap();
        assert_eq!(rows.len(), 3, "event+report = one row, blocked = one, brain = one");
        let fished = rows.iter().find(|row| row.script == "fishing").unwrap();
        assert_eq!(fished.stop, "failed");
        assert_eq!(fished.method, "fishing");
        assert_eq!(fished.position, Some((1, 2, 0)));
        let smith = rows.iter().find(|row| row.script == "smith").unwrap();
        assert_eq!(smith.kind, "Blocked");
        assert_eq!(smith.position, Some((5, 6, 0)));
        assert!(rows.iter().any(|row| row.source == "brain" && row.kind == "graph_stuck"));
    }
}
