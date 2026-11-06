#![forbid(unsafe_code)]
//! Supervision: one thread per account, event telemetry, live state.
//! Methods on daemon::Orchestrator; session control stays in daemon.rs.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::daemon::{Orchestrator, Watchdog, WORKER_TIMEOUT};
use crate::jobs::{Job, now_secs};
use lifescape_json::{Value, object, str_value};

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
    /// One thread per account: accounts are watched in parallel.
    pub(crate) fn start_supervisor(self: &Arc<Self>, job: Arc<Job>) {
        let orch = self.clone();
        std::thread::spawn(move || {
            let mut watchdog = Watchdog::new();
            while orch.running.load(Ordering::SeqCst) && job.inner.lock().unwrap().supervise {
                orch.supervise_once(&job);
                orch.watchdog_tick(&job, &mut watchdog);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }

    fn supervise_once(&self, job: &Arc<Job>) {
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
        let write = {
            let mut inner = job.inner.lock().unwrap();
            if now_secs() - inner.state_written_at >= 2.0 {
                inner.state_written_at = now_secs();
                true
            } else {
                false
            }
        };
        if write {
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
