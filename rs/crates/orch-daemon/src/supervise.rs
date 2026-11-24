#![forbid(unsafe_code)]
//! Supervision: one thread per account, event telemetry, live state.
//! Methods on daemon::Orchestrator; session control stays in daemon.rs.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::daemon::{Orchestrator, Watchdog, WORKER_TIMEOUT};
use crate::fleet::Fleet;
use crate::jobs::{Job, now_secs};
use lifescape_json::{Value, object, str_value};

/// E8 single supervisor: one thread for ALL accounts (was: one thread
/// per account = 300 threads at target). `start_supervisor` only
/// registers; the `supervise-all` thread polls the registry every 500 ms
/// (same cadence, N times less threads) and the watchdog states live in
/// the thread, keyed by account.
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
            let mut guard = crate::jobs::supervised().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.retain(|(_, known)| known.account != job.account);
            let fleet: Arc<dyn Fleet> = self.clone();
            guard.push((fleet, job));
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
    // Last `status` reply per account (lot 4.5: when the next call is due).
    let mut polls: HashMap<String, crate::status_poll::LastPoll> = HashMap::new();
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
        // Liveness for the downtime clock: at most one atomic write a minute.
        crate::downtime::heartbeat(now_secs() as i64);
        if let Some(line) = crate::frame_check::summary_if_due(now_secs()) {
            crate::logln!("{line}");
        }
        if let Some(line) = crate::status_poll::summary_if_due(now_secs()) {
            crate::logln!("{line}");
        }
        let pairs: Vec<(Arc<dyn Fleet>, Arc<Job>)> = {
            let mut guard = crate::jobs::supervised().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            // Never block the loop on a wedged job: a contended inner
            // lock keeps its entry (it rejoins next pass) instead of
            // parking the single supervisor behind an unrelated stall.
            guard.retain(|(orch, job)| {
                if !orch.is_running() {
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
                            crate::logln!(
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
            polls.clear();
            continue;
        }
        let sweep = crate::cpu_profile::Probe::start();
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
            // The registry only ever holds the daemon's own `Orchestrator`.
            let Some(daemon) = orch.as_any().downcast_ref::<Orchestrator>() else { continue };
            let pass_start = crate::jobs::now_secs();
            let once = crate::cpu_profile::Probe::start();
            daemon.supervise_once(job, &mut written, &mut polls);
            crate::cpu_profile::supervise().finish(once, "once");
            let after_once = crate::jobs::now_secs();
            let elapsed = after_once - pass_start;
            if elapsed > 10.0 {
                crate::logln!("brain {}: supervise_once took {:.0}s", job.account, elapsed);
            }
            let wait = backoff_secs(elapsed);
            if wait > 0.0 {
                backed_off.insert(job.account.clone(), after_once + wait);
                crate::logln!("brain {}: backing off {:.0}s after a {:.0}s pass", job.account, wait, elapsed);
            }
            let watchdog = watchdogs.entry(job.account.clone()).or_insert_with(Watchdog::new);
            daemon.watchdog_tick(job, watchdog);
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
                        crate::logln!(
                            "brain {}: inner contended x{hits} at report-drain (~{:.0}s, {:.0}s/pass), job telemetry stalled; watchdog restarts the worker if it fires",
                            job.account, age, hits as f64 / 2.0
                        );
                    }
                    (false, None)
                }
            };
            crate::brain::on_supervise(orch, job, report);
        }
        crate::cpu_profile::supervise().finish(sweep, "sweep");
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

/// True when a FRESH pushed snapshot shows none of the random-event
/// geography (maze/mime rooms, fail tiles, a carried cube): the daemon
/// event it replays is then over. A non-object or stale snapshot says
/// nothing (false).
pub(crate) fn event_is_gone(state: &Value, now: f64) -> bool {
    if state.as_obj().is_none() {
        return false;
    }
    let fresh = state.get("updated").as_f64().is_some_and(|at| now - at < 10.0);
    if !fresh {
        return false;
    }
    let empty: Vec<Value> = Vec::new();
    let pos = state.get("position").as_arr().unwrap_or(&empty);
    let (level, x, z) = (
        pos.first().and_then(|v| v.as_i64()).unwrap_or(-1),
        pos.get(1).and_then(|v| v.as_i64()).unwrap_or(0),
        pos.get(2).and_then(|v| v.as_i64()).unwrap_or(0),
    );
    let present = MAZE_X.0 <= x && x < MAZE_X.1 && MAZE_Z.0 <= z && z < MAZE_Z.1
        || MIME_X.0 <= x && x < MIME_X.1 && MIME_Z.0 <= z && z < MIME_Z.1
        || FAIL_TELEPORTS.contains(&(level, x, z))
        || state.get("inv").as_arr().unwrap_or(&empty).iter().any(|entry| {
            entry.get("name").as_str() == Some("macro_cube") || entry.as_str() == Some("macro_cube")
        });
    !present
}

impl Orchestrator {
    /// E8 single supervisor entry: registered above, polled by the one
    /// `supervise-all` thread. Telemetry still arrives as worker pushes.
    fn supervise_once(&self, job: &Arc<Job>, written: &mut HashMap<String, (f64, String)>, polls: &mut HashMap<String, crate::status_poll::LastPoll>) {
        // Never park the single supervisor on a wedged job: every lock
        // below is a try_lock, contention defers to the next 500 ms pass.
        // A wedged job stalls its own telemetry, never the daemon.
        let tracked = self.procs.lock().unwrap().contains_key(&job.account);
        let alive = !tracked || self.proc_alive(&job.account);
        // Lot 4.5: the `status` round trip only for a silent bot, a
        // transition the frames show, or the minute backstop; the frames
        // carry the phase and status the rest of the time.
        let plan = crate::status_poll::plan(tracked, alive, job.frame_view().as_ref(), polls.get(&job.account), now_secs());
        crate::status_poll::count(plan);
        if plan == crate::status_poll::Plan::Death {
            polls.remove(&job.account);
            self.handle_worker_death(job);
            return;
        }
        let session = if matches!(plan, crate::status_poll::Plan::Status(_)) {
            let frame_before = job.frame_view();
            let call_started = now_secs();
            let reply = self
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
            let previous = polls.get(&job.account).cloned();
            polls.insert(
                job.account.clone(),
                crate::status_poll::LastPoll {
                    at: now_secs(),
                    phase: reply.as_ref().and_then(|session| session.get("phase").as_str().map(str::to_string)).or_else(|| previous.as_ref().map(|poll| poll.phase.clone())).unwrap_or_default(),
                    status: reply.as_ref().and_then(|session| session.get("status").as_str().map(str::to_string)).or_else(|| previous.map(|poll| poll.status)).unwrap_or_default(),
                },
            );
            if let Some(ref session) = reply {
                // Lot 4.2: the frames' phase/status against the reply,
                // silent (counters, summary line), before it is applied.
                crate::frame_check::observe(&job.account, frame_before.as_ref(), job.frame_view().as_ref(), session, call_started, now_secs());
            }
            reply
        } else {
            None
        };
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
        // Read in place under the lock (lot 1.3: no copy of the 35 to 85 KB
        // snapshot for a check that reads a position and a pack). No push
        // yet (daemon just restarted) or wedged job: wait for the next
        // pass instead of parking the supervisor.
        let gone = match job.inner.try_lock() {
            Ok(inner) => match inner.live.as_ref() {
                Some(state) => event_is_gone(state, now_secs()),
                None => return,
            },
            Err(_) => return,
        };
        if !gone {
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
            &object(vec![("cmd", str_value("settle"))]),
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
        inner.phase_at = now_secs();
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
        crate::logln!("job {}: worker died", job.account);
        drop(inner);
        self.write_state(job);
    }
}

#[cfg(test)]
mod tests {
    use super::{backoff_active, backoff_secs, event_is_gone, should_log_contended, CONTEND_LOG_EVERY, CONTEND_LOG_FIRST, FAIL_TELEPORTS, MAZE_X, MAZE_Z, MIME_X, MIME_Z};
    use lifescape_json::{object, str_value, Value};

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

    /// `clear_event_if_gone` as it stood before lot 1.3 (a copy of the
    /// snapshot, `cloned()` position and pack), reduced to its verdict.
    fn event_gone_by_copy(state: Value, now: f64) -> bool {
        if state.as_obj().is_none() {
            return false;
        }
        let fresh = state.get("updated").as_f64().is_some_and(|at| now - at < 10.0);
        if !fresh {
            return false;
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
            present = inv.iter().any(|entry| entry.get("name").as_str() == Some("macro_cube") || entry.as_str() == Some("macro_cube"));
        }
        !present
    }

    /// Lot 1.3: reading the snapshot in place gives the verdict of the
    /// copy, over every geography (maze, mime, each fail tile, plain),
    /// the cube as an object or a bare name, stale or missing `updated`,
    /// missing position, and non-object snapshots.
    #[test]
    fn the_in_place_read_gives_the_verdict_of_the_copy() {
        let now = 1_800_000_000.0;
        let at = |level: i64, x: i64, z: i64| Value::Arr(vec![Value::Int(level), Value::Int(x), Value::Int(z)]);
        let mut positions: Vec<Option<Value>> = vec![None, Some(at(0, 3222, 3218)), Some(at(0, 2900, 4560)), Some(at(0, 2900, 4608)), Some(at(1, 2000, 4750)), Some(at(0, 1983, 4750)), Some(Value::Arr(vec![]))];
        positions.extend(FAIL_TELEPORTS.iter().map(|(level, x, z)| Some(at(*level, *x, *z))));
        positions.extend(FAIL_TELEPORTS.iter().map(|(level, x, z)| Some(at(*level + 1, *x, *z))));
        let packs: Vec<Option<Value>> = vec![
            None,
            Some(Value::Arr(vec![])),
            Some(Value::Arr(vec![object(vec![("name", str_value("coins"))])])),
            Some(Value::Arr(vec![object(vec![("name", str_value("macro_cube"))])])),
            Some(Value::Arr(vec![str_value("macro_cube")])),
            Some(Value::Arr(vec![str_value("coins"), Value::Int(3)])),
        ];
        let updates: Vec<Option<Value>> = vec![None, Some(Value::Float(now - 1.0)), Some(Value::Float(now - 9.9)), Some(Value::Float(now - 10.0)), Some(Value::Float(now - 600.0)), Some(Value::Str("x".to_string()))];
        let (mut checked, mut gone_count) = (0, 0);
        for position in &positions {
            for pack in &packs {
                for update in &updates {
                    let mut fields = vec![("zone", str_value("Lumbridge")), ("trail", Value::Arr(vec![Value::Int(1); 50]))];
                    if let Some(position) = position {
                        fields.push(("position", position.clone()));
                    }
                    if let Some(pack) = pack {
                        fields.push(("inv", pack.clone()));
                    }
                    if let Some(update) = update {
                        fields.push(("updated", update.clone()));
                    }
                    let state = object(fields);
                    let expected = event_gone_by_copy(state.clone(), now);
                    assert_eq!(event_is_gone(&state, now), expected, "{state:?}");
                    checked += 1;
                    gone_count += usize::from(expected);
                }
            }
        }
        for state in [Value::Null, Value::Arr(vec![]), Value::Int(3), Value::Str("s".to_string())] {
            assert_eq!(event_is_gone(&state, now), event_gone_by_copy(state.clone(), now));
        }
        assert!(gone_count > 100 && gone_count < checked, "both verdicts are reached: {gone_count}/{checked}");
    }
}
