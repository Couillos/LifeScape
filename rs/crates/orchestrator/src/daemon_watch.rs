#![forbid(unsafe_code)]
//! Frozen-session watchdog (split from daemon.rs, same behavior):
//! staleness decision plus the supervisor-side tick that restarts wedged
//! workers. The supervisor thread owns the states, keyed by account.

use std::sync::Arc;
use std::time::Duration;

use lifescape_json::{Value, object, str_value};
use crate::daemon::Orchestrator;
use crate::jobs::Job;

/// Frozen-session watchdog: heartbeats older than this (wall clock) mean
/// the worker pumps nothing anymore (wedged thread, dead socket never
/// noticed) no matter what the game ticks say.
const WATCHDOG_STALE_SECS: f64 = 45.0;
/// Poison window: this many watchdog restarts inside it parks the job
/// for the operator instead of respawning forever.
const WATCHDOG_WINDOW_SECS: f64 = 300.0;
const WATCHDOG_MAX_RESTARTS: usize = 3;

/// Frozen-session watchdog state, kept in the supervisor thread (no
/// struct churn): last heartbeat wall time, recent restart times.
pub(crate) struct Watchdog {
    last_hb: Option<f64>,
    restarts: Vec<f64>,
}

impl Watchdog {
    pub(crate) fn new() -> Self {
        Watchdog { last_hb: None, restarts: Vec::new() }
    }
}

/// Pure staleness decision: heartbeat older than the bound (wall clock),
/// never-seen heartbeat never counts (login edge).
pub(crate) fn watchdog_due(last_hb: Option<f64>, now: f64) -> bool {
    match last_hb {
        Some(at) => now - at >= WATCHDOG_STALE_SECS,
        None => false,
    }
}

impl Orchestrator {
    /// Frozen-session watchdog, called from the supervisor thread: when
    /// heartbeats stop arriving although the session should be live, the
    /// worker pumps nothing anymore (wedged thread, dead socket never
    /// noticed) and no in-worker timeout can fire. Kill the worker,
    /// respawn it, and resume the running script if any (stateless
    /// scripts restart cleanly from anywhere). Poison-capped: repeated
    /// wedges park the job loudly instead of respawning forever.
    pub(crate) fn watchdog_tick(&self, job: &Arc<Job>, state: &mut Watchdog) {
        let (phase, script, args, status, paused, held, updated) = {
            let Ok(inner) = job.inner.try_lock() else {
                return;
            };
            let updated = inner.live.as_ref().and_then(|frame| frame.get("updated").as_f64());
            (
                inner.phase.clone(),
                inner.script.clone(),
                inner.script_args.clone(),
                inner.status.clone(),
                inner.paused,
                inner.held,
                updated,
            )
        };
        if paused || held {
            return;
        }
        if matches!(status.as_str(), "reconnecting" | "disconnected" | "starting" | "busy") {
            return;
        }
        let Some(at) = updated else {
            return;
        };
        let now = crate::jobs::now_secs();
        if at > state.last_hb.unwrap_or(0.0) {
            state.last_hb = Some(at);
            return;
        }
        if !watchdog_due(state.last_hb, now) {
            return;
        }
        state.restarts.retain(|seen| now - seen < WATCHDOG_WINDOW_SECS);
        if state.restarts.len() >= WATCHDOG_MAX_RESTARTS {
            if let Ok(mut inner) = job.inner.try_lock() {
                inner.last_error = Some(object(vec![
                    ("where", str_value("watchdog")),
                    ("type", str_value("watchdog")),
                    ("message", str_value("session wedged repeatedly, parked for operator")),
                    ("at", Value::Arr(vec![])),
                ]));
            }
            return;
        }
        state.restarts.push(now);
        println!("watchdog {}: no heartbeat for {:.0}s, restarting worker", job.account, now - at);
        self.kill_process(&job.account);
        std::thread::sleep(Duration::from_millis(1500));
        if let Err(error) = self.spawn(job, false) {
            println!("watchdog {}: respawn failed: {error}", job.account);
            return;
        }
        let deadline = crate::jobs::now_secs() + 15.0;
        let mut ready = false;
        while crate::jobs::now_secs() < deadline {
            match self.send_worker(&job.account, &object(vec![("cmd", str_value("status"))]), Duration::from_secs(3)) {
                Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
                    ready = true;
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        if !ready {
            println!("watchdog {}: respawned worker never answered", job.account);
            return;
        }
        if phase == "running" {
            if let Some(name) = script {
                println!("watchdog {}: resuming {name} after restart", job.account);
                self.run_job(&job.account, &name, &args);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::watchdog_due;

    #[test]
    fn watchdog_fires_only_on_stale_heartbeat() {
        assert!(!watchdog_due(None, 1000.0), "never seen is login edge, not death");
        assert!(!watchdog_due(Some(990.0), 1000.0), "10s fresh is healthy");
        assert!(watchdog_due(Some(900.0), 1000.0), "100s stale is wedged");
    }
}
