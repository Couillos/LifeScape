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

/// The one supervision loop: poll every account, prune parked/closed
/// jobs, drain report flags (brain trigger hook) immediately.
fn supervise_all() {
    let mut watchdogs: HashMap<String, Watchdog> = HashMap::new();
    let mut written: HashMap<String, (f64, String)> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let pairs: Vec<(Arc<Orchestrator>, Arc<Job>)> = {
            let mut guard = registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.retain(|(orch, job)| {
                orch.running.load(Ordering::SeqCst) && job.inner.lock().unwrap().supervise
            });
            guard.clone()
        };
        if pairs.is_empty() {
            watchdogs.clear();
            written.clear();
            continue;
        }
        for (orch, job) in &pairs {
            orch.supervise_once(job, &mut written);
            let watchdog = watchdogs.entry(job.account.clone()).or_insert_with(Watchdog::new);
            orch.watchdog_tick(job, watchdog);
            // E8 brain trigger: a pushed task report wakes handling on
            // the next pass instead of waiting for a poll. The brain
            // itself lands in a later lot; the flag guarantees no report
            // waits longer than one 500 ms pass.
            if std::mem::replace(&mut job.inner.lock().unwrap().report_pending, false) {
                // Rendements: one TaskReport in, raw store rows plus the
                // persisted snapshot buckets out. Loud on failure, never
                // a silent drop (the staging log keeps the raw report).
                let report = job.inner.lock().unwrap().last_report.clone();
                if let Some(report) = report {
                    if let Err(error) = crate::jobs::ingest_report_persisted(&report) {
                        println!("job {}: rates ingest failed: {error}", job.account);
                    }
                }
                let _ = &written;
            }
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
            let inner = job.inner.lock().unwrap();
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
        let state = match job.inner.lock().unwrap().live.clone() {
            Some(state) => state,
            // No push yet (daemon just restarted): wait for the next one.
            None => return,
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
        let mut inner = job.inner.lock().unwrap();
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
            let inner = job.inner.lock().unwrap();
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
                let mut inner = job.inner.lock().unwrap();
                if inner.status == "finished" {
                    inner.status = "idle".to_string();
                }
            }
        }
    }

    fn handle_worker_death(&self, job: &Arc<Job>) {
        self.procs.lock().unwrap().remove(&job.account);
        let mut inner = job.inner.lock().unwrap();
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
