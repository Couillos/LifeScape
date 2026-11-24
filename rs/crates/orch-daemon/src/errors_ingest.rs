#![forbid(unsafe_code)]
//! Error collector (errors page): one daemon thread feeds the
//! `error_events` table of the store.
//!
//! Pushed sources go through a bounded channel, never blocking the
//! caller: script error events and failed/blocked task reports (from
//! `ingest_push`), brain lines (from `logln!`). Pulled sources are
//! rescanned every 30 s: nav dossiers (`run/failures/*.json`, by mtime;
//! incremental: only names never seen are stat'ed, a full stat pass runs
//! hourly to catch a name rewritten in place) and failed deals (`deals` table). Pulled rows carry a unique
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

pub use crate::errors_model::RETENTION_SECS;
const CHANNEL_CAP: usize = 8_192;
const BATCH_ROWS: usize = 200;
const BATCH_AGE: Duration = Duration::from_secs(2);
const SCAN_EVERY: Duration = Duration::from_secs(30);
const PURGE_EVERY: Duration = Duration::from_secs(3_600);
/// Full dossier pass (stat every file): catches a dossier rewritten under
/// a name already seen (`{account}-{tick}` after a tick reset).
const FULL_SCAN_EVERY: Duration = Duration::from_secs(3_600);
/// A dossier younger than this is re-examined next scan (it may still
/// be being written); older ones are remembered as seen.
const SETTLE_SECS: i64 = 2;
/// Pending intake kept across store failures before dropping (logged).
const PENDING_CAP: usize = 20_000;
/// A dossier this fresh takes its task from the job's current run.
const FRESH_DOSSIER_SECS: i64 = 90;

enum Intake {
    Event { account: String, frame: Value, clock: i64 },
    Report { account: String, report: Value, position: Option<(i64, i64, i64)>, clock: i64 },
    /// A report the silent-loop detector flags (`report_progress`).
    Idle { account: String, report: Value, position: Option<(i64, i64, i64)>, clock: i64 },
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
pub fn observe_event(account: &str, frame: &crate::frame_codec::EventFrame) {
    if frame.kind != "script_error" {
        return;
    }
    send(Intake::Event { account: account.to_string(), frame: frame.encode(), clock: now_clock() });
}

/// A pushed task report; only failed/blocked stops become rows. The
/// position is the failure's own (`last_error.at` when it is the same
/// message), else the live one. Call before locking the job.
pub fn observe_report(account: &str, report: &Value, job: &Job) {
    let stop = report.get("stop").as_str().unwrap_or("");
    // A silent loop (window without output, bank churn) is a row of its
    // own, never a quiet `budget_exhausted` (2026-11-14, pottery loops).
    if crate::report_progress::assess(report).is_some() {
        let position = job.inner.try_lock().ok().and_then(|inner| inner.live.as_ref().and_then(|live| errors_model::position_of(live.get("position"))));
        send(Intake::Idle { account: account.to_string(), report: report.clone(), position, clock: now_clock() });
    }
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

/// Registers `observe_log_line` as the `logln!` sink (`log_sink`); the
/// binary calls it first thing in `main`, tests that need the idle
/// reasons call it too.
pub fn register_log_sink() {
    crate::log_sink::register(observe_log_line);
}

/// Hook of every daemon log line (`logln!`): brain abandons, stuck
/// graphs and goals, bans. Cheap prefix test first; a no-op until the
/// collector runs (workers and CLI never start it).
pub fn observe_log_line(line: &str) {
    if !line.starts_with("brain ") {
        return;
    }
    // The idle detector's reasons: the account's last brain decisions.
    crate::autopilot::current().remember_line(line);
    if SENDER.get().is_none() {
        return;
    }
    let wanted = line.contains(" -> abandoned (")
        || line.contains(": graph stuck")
        || line.contains(": goal stuck")
        || line.contains(" banned ")
        || line.contains(": revive cap reached")
        || line.contains(" no progress: ")
        || crate::autopilot::current().is_collected_brain_line(line);
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
    crate::paths::store_db()
}

fn failures_dir() -> PathBuf {
    crate::paths::run_dir().join("failures")
}

/// Runs a store job on the process's single writer (grouped commit) when
/// the daemon registered one, else on a pooled connection.
fn through_writer<T>(job: impl FnOnce(&Store) -> Result<T, String>) -> Result<T, String> {
    crate::store_writer::write_to(&db_path(), job).map_err(|error| format!("errors store open: {error}"))?
}

fn collect(receiver: Receiver<Intake>, orch: Weak<Orchestrator>) {
    let mut pending: Vec<Intake> = Vec::new();
    let mut last_flush = Instant::now();
    let mut last_scan: Option<Instant> = None;
    let mut last_purge: Option<Instant> = None;
    let mut dossier_cursor = DossierCursor::default();
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
            let result = through_writer(|store| flush(store, &pending, &orch));
            match result {
                Ok(()) => pending.clear(),
                Err(error) => {
                    crate::logln!("errors: flush of {} rows failed: {error}", pending.len());
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
            let result = through_writer(|store| {
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
                }
            }
        }
        if last_purge.is_none_or(|at| at.elapsed() >= PURGE_EVERY) {
            last_purge = Some(Instant::now());
            match through_writer(|store| store.purge_errors_before(now_clock() - RETENTION_SECS).map_err(|error| error.to_string())) {
                Ok(removed) if removed > 0 => crate::logln!("errors: purged {removed} rows past retention"),
                Ok(_) => {}
                Err(error) => crate::logln!("errors: purge failed: {error}"),
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
            Intake::Idle { account, report, position, clock } => {
                if let Some(row) = errors_model::from_idle_report(account, report, *position, *clock, zone_of) {
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

fn name_hash(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

/// Scan state across passes (CPU 2026-11-14: `run/failures` holds ~60k
/// dossiers, and each 30 s pass used to stat every one of them).
/// `clock`: newest dossier mtime pulled. `seen`: hashes of the names
/// already examined (8 bytes each), skipped without a stat until the
/// next hourly full pass, which rebuilds the set from the directory.
#[derive(Default)]
pub(crate) struct DossierCursor {
    clock: Option<i64>,
    seen: std::collections::HashSet<u64>,
    last_full: Option<Instant>,
    /// Files stat'ed by the last pass (the cost this cursor bounds).
    examined: usize,
}

/// Dossiers written at or after the cursor (first pass: the newest nav
/// row, else the retention start), oldest first, in batches of 500.
/// Between hourly full passes only never-seen names are stat'ed.
pub(crate) fn scan_dossiers(
    store: &Store,
    dir: &Path,
    cursor: &mut DossierCursor,
    orch: &Weak<Orchestrator>,
) -> Result<usize, String> {
    let full = cursor.clock.is_none() || cursor.last_full.is_none_or(|at| at.elapsed() >= FULL_SCAN_EVERY);
    let start = match cursor.clock {
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
    let now = now_clock();
    let mut seen = if full { std::collections::HashSet::new() } else { std::mem::take(&mut cursor.seen) };
    cursor.examined = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") {
            continue;
        }
        let hash = name_hash(&name);
        if !full && seen.contains(&hash) {
            continue;
        }
        cursor.examined += 1;
        if let Some(mtime) = mtime_of(&entry) {
            if now - mtime > SETTLE_SECS {
                seen.insert(hash);
            }
            // One second of slack: a file written in the cursor's second
            // may have been missed; the unique ref drops the repeat.
            if mtime >= start - 1 {
                fresh.push((mtime, name));
            }
        }
    }
    cursor.seen = seen;
    if full {
        cursor.last_full = Some(Instant::now());
    }
    fresh.sort();
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
            // A walled-in origin strands the bot (P2-1a, `brain_stranded`).
            if row.kind == "NoRoute" {
                let dest = dossier.get("dest");
                let dest = dest.get("x").as_i64().zip(dest.get("z").as_i64()).map(|(x, z)| (x, z, dest.get("level").as_i64().unwrap_or(0)));
                crate::autopilot::current().observe_no_route(&row.account, row.position, dest, &row.message);
            }
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
    cursor.clock = Some(newest);
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the brain lines the sink hands to the autopilot.
    struct Recorder;

    static HEARD: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    impl lifescape_orch_fleet::autopilot::Autopilot for Recorder {
        fn remember_line(&self, line: &str) {
            HEARD.lock().unwrap().push(line.to_string());
        }
    }

    /// The idle-reason recording rides on the `logln!` sink: once the
    /// binary's registration ran (`register_log_sink`, first thing in
    /// `main`), a brain line logged anywhere reaches the autopilot
    /// (`remember_line`, the brain's `brain_idle`). A missing registration
    /// would silently stop the reasons.
    #[test]
    fn logln_reaches_the_idle_reasons_through_the_registered_sink() {
        register_log_sink();
        crate::log_sink::assert_registered();
        lifescape_orch_fleet::autopilot::register(Arc::new(Recorder));
        crate::logln!("brain sink_probe: no route to the bank");
        assert!(HEARD.lock().unwrap().iter().any(|line| line.contains("sink_probe: no route to the bank")));
    }

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
        let mut cursor = DossierCursor::default();
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 1);
        assert!(cursor.clock.is_some());
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0, "rescan adds nothing");
        let mut fresh_cursor = DossierCursor::default();
        assert_eq!(scan_dossiers(&store, &dir, &mut fresh_cursor, &orch).unwrap(), 0, "restart adds nothing");
        let rows = store.errors_matching(&lifescape_store::ErrorFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dossier, "t01-5.json");
        let missing = dir.join("absent");
        assert_eq!(scan_dossiers(&store, &missing, &mut DossierCursor::default(), &orch).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn age(path: &Path, secs: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    /// Incremental passes stat only never-seen names: a settled directory
    /// costs zero stats, a new dossier costs one and lands once. The
    /// hourly full pass stats everything again (rewrites in place).
    #[test]
    fn dossier_scan_stats_only_new_names() {
        let dir = std::env::temp_dir().join(format!("lifescape-errors-incr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dossier = r#"{"account":"t01","from":{"level":0,"x":10,"z":20},"error":{"kind":"NoRoute","observed":"travel: no route"}}"#;
        for tick in 0..50 {
            let path = dir.join(format!("t01-{tick}.json"));
            std::fs::write(&path, dossier).unwrap();
            age(&path, 10);
        }
        let store = Store::open_in_memory().unwrap();
        let orch: Weak<Orchestrator> = Weak::new();
        let mut cursor = DossierCursor::default();
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 50);
        assert_eq!(cursor.examined, 50, "first pass is full");
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0);
        assert_eq!(cursor.examined, 0, "settled directory: no stat at all");
        let path = dir.join("t02-7.json");
        std::fs::write(&path, dossier.replace("t01", "t02")).unwrap();
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 1, "new dossier lands");
        assert_eq!(cursor.examined, 1, "only the new name is stat'ed");
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0, "fresh file re-read, deduped");
        assert_eq!(cursor.examined, 1, "a still-fresh file is re-examined until it settles");
        age(&path, 10);
        scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap();
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0);
        assert_eq!(cursor.examined, 0, "settled again");
        cursor.last_full = None;
        assert_eq!(scan_dossiers(&store, &dir, &mut cursor, &orch).unwrap(), 0, "full pass adds nothing new");
        assert_eq!(cursor.examined, 51, "full pass stats everything");
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
