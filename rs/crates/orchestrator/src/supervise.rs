#![forbid(unsafe_code)]
//! Supervision: one thread per account, event telemetry, live state.
//! Methods on daemon::Orchestrator; session control stays in daemon.rs.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::daemon::{Orchestrator, Watchdog, WORKER_TIMEOUT};
use crate::jobs::{Job, now_secs};
use lifescape_json::{Value, object, str_value};

/// E8 single supervisor: one thread for ALL accounts (was: one thread
/// per account = 300 threads at target). `start_supervisor` only
/// registers; the `supervise-all` thread polls the registry every 500 ms
/// (same cadence, N times less threads) and the watchdog states live in
/// the thread, keyed by account.
fn registry() -> &'static Mutex<Vec<(Arc<Orchestrator>, Arc<Job>)>> {
    static REGISTRY: OnceLock<Mutex<Vec<(Arc<Orchestrator>, Arc<Job>)>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

fn ensure_supervisor_thread() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("supervise-all".to_string())
            .spawn(supervise_all)
            .expect("supervisor thread spawns");
    });
}

/// Registration only: the single thread does the polling. Re-registering
/// an account replaces the stale entry (re-open after a dead worker).
impl Orchestrator {
    pub(crate) fn start_supervisor(self: &Arc<Self>, job: Arc<Job>) {
        {
            let mut guard = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.retain(|(_, known)| known.account != job.account);
            guard.push((self.clone(), job));
        }
        ensure_supervisor_thread();
    }
}

/// Defaut 5, verrou fantome: holder instrumentation for every `inner`
/// lock on the hot path. Each `try_lock` site names its op; on
/// contention the log names the wedged account, the failing op and how
/// long it has been failing, instead of "holder unknown". Pure helpers
/// below keep the thresholds testable; the maps stay local to the one
/// supervisor thread (no new shared lock to wedge).
pub const CONTEND_LOG_FIRST: u32 = 10;
pub const CONTEND_LOG_EVERY: u32 = 60;

/// Log on the 10th consecutive contention then every 60 passes (30 s):
/// names the wedged account instead of stalling silently.
pub fn should_log_contended(hits: u32) -> bool {
    hits == CONTEND_LOG_FIRST || (hits >= CONTEND_LOG_EVERY && hits % CONTEND_LOG_EVERY == 0)
}

/// Circuit breaker: a `supervise_once` past 5 s backs the account off
/// 30 s, past 10 s backs it off 60 s. The single supervisor then skips
/// the blocking `send_worker` RPC for that account (telemetry deferred,
/// watchdog still runs) instead of queueing 5 s stalls behind one wedge.
pub fn backoff_secs(elapsed: f64) -> f64 {
    if elapsed > 10.0 {
        60.0
    } else if elapsed > 5.0 {
        30.0
    } else {
        0.0
    }
}

pub fn backoff_active(until: f64, now: f64) -> bool {
    now < until
}

/// The one supervision loop: poll every account, prune parked/closed
/// jobs, drain report flags (brain trigger hook) immediately.
fn supervise_all() {
    let mut watchdogs: HashMap<String, Watchdog> = HashMap::new();
    let mut written: HashMap<String, (f64, String)> = HashMap::new();
    // Consecutive contentions per account plus the failing op and the
    // first-failure time: the holder trace that isolates the verrou
    // fantome to one site instead of "holder unknown". Resets on the
    // first clean drain. Circuit-breaker deadlines skip the blocking
    // worker RPC while a wedge cools down.
    let mut contended: HashMap<String, u32> = HashMap::new();
    let mut contended_op: HashMap<String, &'static str> = HashMap::new();
    let mut contended_since: HashMap<String, f64> = HashMap::new();
    let mut backed_off: HashMap<String, f64> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let pairs: Vec<(Arc<Orchestrator>, Arc<Job>)> = {
            let mut guard = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            // Never block the loop on a wedged job: a contended inner
            // lock keeps its entry (it rejoins next pass) instead of
            // parking the single supervisor behind an unrelated stall.
            guard.retain(|(orch, job)| {
                if !(orch.running.load(Ordering::SeqCst)) {
                    return false;
                }
                match job.inner.try_lock() {
                    Ok(inner) => {
                        if !inner.supervise {
                            return false;
                        }
                        true
                    }
                    Err(_) => {
                        // Registry scan is the first hot-path lock: trace
                        // it like the drain below so a wedge here names
                        // itself instead of vanishing from the loop.
                        let now = crate::jobs::now_secs();
                        let hits = contended.get(&job.account).cloned().unwrap_or(0) + 1;
                        contended.insert(job.account.clone(), hits);
                        contended_op.insert(job.account.clone(), "registry-retain");
                        contended_since.entry(job.account.clone()).or_insert(now);
                        if should_log_contended(hits) {
                            let age = now - contended_since.get(&job.account).copied().unwrap_or(now);
                            println!(
                                "brain {}: inner contended x{hits} at registry-retain (~{:.0}s), telemetry stalled; watchdog restarts the worker if it fires",
                                job.account, age
                            );
                        }
                        true
                    }
                }
            });
            guard.clone()
        };
        if pairs.is_empty() {
            watchdogs.clear();
            written.clear();
            continue;
        }
        for (orch, job) in &pairs {
            let now = crate::jobs::now_secs();
            // Circuit breaker: skip the blocking RPC while cooling down.
            // The watchdog below still runs, so a dead worker restarts;
            // the brain tick still enqueues (it never blocks).
            if let Some(until) = backed_off.get(&job.account) {
                if backoff_active(*until, now) {
                    let (_had_report, report) = match job.inner.try_lock() {
                        Ok(mut inner) => {
                            let pending = std::mem::replace(&mut inner.report_pending, false);
                            let report = if pending { inner.last_report.clone() } else { None };
                            contended.remove(&job.account);
                            contended_op.remove(&job.account);
                            contended_since.remove(&job.account);
                            (pending, report)
                        }
                        Err(_) => (false, None),
                    };
                    crate::brain::on_supervise(orch, job, report);
                    continue;
                } else {
                    backed_off.remove(&job.account);
                }
            }
            let pass_start = crate::jobs::now_secs();
            orch.supervise_once(job, &mut written);
            let after_once = crate::jobs::now_secs();
            let elapsed = after_once - pass_start;
            if elapsed > 10.0 {
                println!("brain {}: supervise_once took {:.0}s", job.account, elapsed);
            }
            let wait = backoff_secs(elapsed);
            if wait > 0.0 {
                backed_off.insert(job.account.clone(), after_once + wait);
                println!("brain {}: backing off {:.0}s after a {:.0}s pass", job.account, wait, elapsed);
            }
            let watchdog = watchdogs.entry(job.account.clone()).or_insert_with(Watchdog::new);
            orch.watchdog_tick(job, watchdog);
            // E8 brain trigger: a pushed task report wakes handling on
            // the next pass instead of waiting for a poll. The brain
            // folds it into the life graph, then launches the next node.
            let (_had_report, report) = match job.inner.try_lock() {
                // A wedged job must not park the single supervisor: skip
                // this pass, the flag waits for the next one.
                Ok(mut inner) => {
                    let pending = std::mem::replace(&mut inner.report_pending, false);
                    let report = if pending { inner.last_report.clone() } else { None };
                    contended.remove(&job.account);
                    contended_op.remove(&job.account);
                    contended_since.remove(&job.account);
                    (pending, report)
                }
                Err(_) => {
                    let at = crate::jobs::now_secs();
                    let hits = contended.get(&job.account).cloned().unwrap_or(0) + 1;
                    contended.insert(job.account.clone(), hits);
                    contended_op.insert(job.account.clone(), "report-drain");
                    contended_since.entry(job.account.clone()).or_insert(at);
                    if should_log_contended(hits) {
                        let age = at - contended_since.get(&job.account).copied().unwrap_or(at);
                        println!(
                            "brain {}: inner contended x{hits} at report-drain (~{:.0}s, {:.0}s/pass), job telemetry stalled; watchdog restarts the worker if it fires",
                            job.account, age, hits as f64 / 2.0
                        );
                    }
                    (false, None)
                }
            };
            crate::brain::on_supervise(orch, job, report);
        }
    }
}

// Random-event geography, decoded from the streamed random events.
const MAZE_X: (i64, i64) = (2880, 2944);
const MAZE_Z: (i64, i64) = (4544, 4608);
const MIME_X: (i64, i64) = (1984, 2048);
const MIME_Z: (i64, i64) = (4736, 4800);
const FAIL_TELEPORTS: &[(i64, i64, i64)] = &[
    (1, 3278, 3500), (1, 3097, 3268), (2, 2954, 3388), (3, 3221, 3474),
    (1, 3229, 3223), (0, 3097, 9567), (2, 3287, 3190), (1, 2964, 3213),
    (0, 2968, 3335), (0, 2935, 3515), (0, 3212, 3423), (0, 3076, 3412),
    (0, 2996, 3145), (1, 3025, 3251), (1, 2975, 3382), (1, 3214, 3215),
    (1, 3112, 3157), (0, 3246, 3451), (0, 3283, 3218), (0, 3045, 3491),
    (0, 3083, 3469), (2, 3104, 3163), (0, 3218, 9618), (0, 3263, 3424),
];

impl Orchestrator {
    /// E8 single supervisor entry: registered above, polled by the one
    /// `supervise-all` thread. Telemetry still arrives as worker pushes.
    fn supervise_once(&self, job: &Arc<Job>, written: &mut HashMap<String, (f64, String)>) {
        // Never park the single supervisor on a wedged job: every lock
        // below is a try_lock, contention defers to the next 500 ms pass.
        // A wedged job stalls its own telemetry, never the daemon.
        if self.procs.lock().unwrap().contains_key(&job.account) && !self.proc_alive(&job.account) {
            self.handle_worker_death(job);
            return;
        }
        let session = self
            .send_worker(
                &job.account,
                &object(vec![("cmd", str_value("status"))]),
                Duration::from_secs(5),
            )
            .ok()
            .and_then(|reply| {
                if reply.get("session").is_null() {
                    None
                } else {
                    Some(reply.get("session").clone())
                }
            });
        if let Some(ref session) = session {
            // apply_session try_locks internally: a wedged job defers to
            // the next pass instead of parking the supervisor here.
            job.apply_session(session);
        }
        // Telemetry arrives as worker push frames (ingest_push): the worker
        // poses its rescue inline, the daemon never unloads, runs or resumes
        // anything.
        self.clear_event_if_gone(job);
        self.auto_idle_finished(job);
        // E4+E8: merged state writes ride the slow cadence (30 s) or a
        // visible change (phase/status/script), not every 2 s per bot.
        // At 300 bots that is ~10 files/s worst case, ~0 idle.
        let (key, due) = {
            let Ok(inner) = job.inner.try_lock() else {
                return;
            };
            let key = format!("{}|{}|{:?}", inner.phase, inner.status, inner.script);
            let seen = written.get(&job.account);
            let changed = seen.map(|(_, old)| old != &key).unwrap_or(true);
            let elapsed = seen.map(|(at, _)| now_secs() - at).unwrap_or(f64::MAX);
            (key, changed || elapsed >= 30.0)
        };
        if due {
            written.insert(job.account.clone(), (now_secs(), key));
            self.write_state(job);
        }
    }

    /// The rescue ran: drop the daemon event when the pushed live state
    /// shows none (maze/mime rooms, fail tiles, carried cube). Rescues
    /// replay a stale unresolved event every tick otherwise.
    fn clear_event_if_gone(&self, job: &Arc<Job>) {
        let state = match job.inner.try_lock().map(|inner| inner.live.clone()) {
            Ok(Some(state)) => state,
            // No push yet (daemon just restarted) or wedged job: wait
            // for the next pass instead of parking the supervisor.
            _ => return,
        };
        if state.as_obj().is_none() {
            return;
        }
        let fresh = state
            .get("updated")
            .as_f64()
            .is_some_and(|at| now_secs() - at < 10.0);
        if !fresh {
            return;
        }
        let pos = state.get("position").as_arr().cloned().unwrap_or_default();
        let (level, x, z) = (
            pos.first().and_then(|v| v.as_i64()).unwrap_or(-1),
            pos.get(1).and_then(|v| v.as_i64()).unwrap_or(0),
            pos.get(2).and_then(|v| v.as_i64()).unwrap_or(0),
        );
        let mut present = MAZE_X.0 <= x && x < MAZE_X.1 && MAZE_Z.0 <= z && z < MAZE_Z.1
            || MIME_X.0 <= x && x < MIME_X.1 && MIME_Z.0 <= z && z < MIME_Z.1
            || FAIL_TELEPORTS.contains(&(level, x, z));
        if !present {
            let inv = state.get("inv").as_arr().cloned().unwrap_or_default();
            present = inv.iter().any(|entry| {
                entry.get("name").as_str() == Some("macro_cube") || entry.as_str() == Some("macro_cube")
            });
        }
        if present {
            return;
        }
        let Ok(mut inner) = job.inner.try_lock() else {
            return;
        };
        if let Some(Value::Obj(mut obj)) = inner.last_event.take() {
            if obj.get("resolved").and_then(|v| v.as_bool()) != Some(true) {
                obj.insert("resolved".to_string(), Value::Bool(true));
            }
            inner.last_event = Some(Value::Obj(obj));
        }
    }

    /// Finished is transient: unload it back to idle.
    fn auto_idle_finished(&self, job: &Arc<Job>) {
        {
            let Ok(inner) = job.inner.try_lock() else {
                return;
            };
            if inner.phase != "idle" || inner.status != "finished" {
                return;
            }
        }
        let reply = self.send_worker(
            &job.account,
            &object(vec![("cmd", str_value("unload"))]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        if let Ok(reply) = reply {
            if reply.get("ok").as_bool() == Some(true) {
                job.apply_session(reply.get("session"));
                if let Ok(mut inner) = job.inner.try_lock() {
                    if inner.status == "finished" {
                        inner.status = "idle".to_string();
                    }
                }
            }
        }
    }

    fn handle_worker_death(&self, job: &Arc<Job>) {
        self.procs.lock().unwrap().remove(&job.account);
        let Ok(mut inner) = job.inner.try_lock() else {
            return;
        };
        inner.supervise = false;
        inner.phase = "idle".to_string();
        inner.script = None;
        inner.status = "disconnected".to_string();
        inner.last_error = Some(object(vec![
            ("where", str_value("worker")),
            ("type", str_value("death")),
            (
                "message",
                str_value("session worker died: close + open to recover"),
            ),
            ("at", Value::Arr(vec![])),
        ]));
        println!("job {}: worker died", job.account);
        drop(inner);
        self.write_state(job);
    }
}

#[cfg(test)]
mod tests {
    use super::{backoff_active, backoff_secs, should_log_contended, CONTEND_LOG_EVERY, CONTEND_LOG_FIRST};

    /// Defaut 5: contention names itself on a fixed cadence (10, 60,
    /// 120...), never silently and never every pass.
    #[test]
    fn contention_log_cadence_is_pinned() {
        assert_eq!((CONTEND_LOG_FIRST, CONTEND_LOG_EVERY), (10, 60));
        assert!(should_log_contended(10), "first report at x10");
        assert!(!should_log_contended(11), "quiet between reports");
        assert!(should_log_contended(60) && should_log_contended(120), "then every 60");
        assert!(!should_log_contended(9) && !should_log_contended(1), "early wedges stay quiet");
    }

    /// Circuit breaker: slow passes back off, fast passes never do.
    #[test]
    fn slow_passes_back_off_fast_passes_do_not() {
        assert_eq!(backoff_secs(4.9), 0.0, "fast pass keeps polling");
        assert_eq!(backoff_secs(5.1), 30.0, "5 s stall cools down 30 s");
        assert_eq!(backoff_secs(10.1), 60.0, "10 s wedge cools down 60 s");
        assert!(backoff_active(100.0, 99.9), "deadline in the future skips the RPC");
        assert!(!backoff_active(100.0, 100.0), "expired deadline polls again");
        assert!(!backoff_active(100.0, 101.0), "past deadline polls again");
    }
}
