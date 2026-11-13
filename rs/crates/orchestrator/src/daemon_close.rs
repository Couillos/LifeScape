#![forbid(unsafe_code)]
//! Session end and redeploy (split from daemon.rs): `close` ends every
//! worker of the account, tracked or orphaned, and says how; `redeploy`
//! closes then reopens under one lifecycle claim, so no other open
//! (market-absent, brain) slips in between.

use std::sync::Arc;
use std::time::Duration;

use crate::daemon::{Orchestrator, WorkerProc};
use crate::paths;
use crate::worker_owner;
use lifescape_json::{Value, object, str_value};

/// Bounded wait for a worker that acknowledged `close` to log out and
/// exit on its own (its script stops at the next pump, then logout),
/// wall seconds: process-side bound, outside any game connection.
pub const CLOSE_WAIT_SECS: f64 = 20.0;
/// Redeploy keeps retrying the login while the server still holds the
/// dropped player: LostCity forces the logout after 100 ticks without a
/// response (`World.TIMEOUT_NO_RESPONSE`), plus margin.
pub const RELOGIN_TICKS: u64 = 120;
/// Spacing of those login retries.
const RELOGIN_RETRY_SECS: f64 = 2.0;

fn refusal(error: &str) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(error))])
}

/// Why a redeploy reopen may be retried, and after how long: the open
/// pace (`admission:open-rate wait Ns`) or the server still holding the
/// player of the worker just closed (login code 5). Anything else is a
/// real failure, reported as is.
pub fn reopen_retry_secs(error: &str) -> Option<f64> {
    if let Some(rest) = error.strip_prefix("admission:open-rate wait ") {
        return rest.trim_end_matches('s').parse::<f64>().ok().map(|wait| wait.max(0.1));
    }
    if error.contains("already logged in") {
        return Some(RELOGIN_RETRY_SECS);
    }
    None
}

impl Orchestrator {
    pub(crate) fn close_job(&self, account: &str) -> Value {
        let Some(_claim) = self.lifecycle.try_claim(account) else {
            return refusal(&format!("busy {account}: an open/close is in flight, retry"));
        };
        self.close_claimed(account)
    }

    /// End every worker of `account`, lifecycle claim held by the caller:
    /// ask for a graceful logout (acked at once, honoured at the worker's
    /// next pump), wait `CLOSE_WAIT_SECS` for the exit, then SIGTERM /
    /// SIGKILL the tracked worker and any untracked one (`/proc` truth).
    /// `ok` only once no worker of the account is left alive.
    pub(crate) fn close_claimed(&self, account: &str) -> Value {
        let job = self.jobs.lock().unwrap().get(account).cloned();
        let tracked = self.procs.lock().unwrap().get_mut(account).and_then(WorkerProc::pid);
        let mut pids = worker_owner::live_workers(account);
        if let Some(pid) = tracked {
            if !pids.contains(&pid) && worker_owner::pid_alive(pid) {
                pids.push(pid);
            }
        }
        if job.is_none() && pids.is_empty() {
            self.procs.lock().unwrap().remove(account);
            return refusal(&format!("no session for {account}"));
        }
        if let Some(job) = &job {
            if let Ok(mut inner) = job.inner.try_lock() {
                inner.supervise = false;
            }
        }
        let mut session = Value::Null;
        let mut acked = false;
        match self.send_worker(account, &object(vec![("cmd", str_value("close"))]), Duration::from_secs(5)) {
            Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
                session = reply.get("session").clone();
                acked = true;
            }
            Ok(reply) => crate::logln!("close {account}: worker refused ({})", reply.get("error").as_str().unwrap_or("?")),
            Err(error) => crate::logln!("close {account}: worker unreachable: {error}"),
        }
        let graceful = acked && worker_owner::wait_gone(&pids, CLOSE_WAIT_SECS);
        self.kill_process(account);
        let mut terminated = Vec::new();
        for pid in worker_owner::live_workers(account) {
            crate::logln!("close {account}: terminating untracked worker pid {pid}");
            worker_owner::terminate_pid(pid);
            terminated.push(pid);
        }
        let survivors = worker_owner::live_workers(account);
        if survivors.is_empty() {
            let (sock, _ready) = paths::session_paths(account);
            let _ = std::fs::remove_file(sock);
        }
        if let Some(job) = &job {
            {
                let mut inner = job.inner.lock().unwrap();
                inner.status = "stopped".to_string();
                inner.phase = "idle".to_string();
                inner.script = None;
                inner.paused = false;
                inner.supervise = false;
                if !session.get("last_error").is_null() {
                    inner.last_error = Some(session.get("last_error").clone());
                }
            }
            self.write_state(job);
        }
        self.jobs.lock().unwrap().remove(account);
        let how = if graceful { "logged out" } else { "terminated" };
        crate::logln!("close {account}: {how}, workers {pids:?} (untracked {terminated:?}), survivors {survivors:?}");
        let pid_list = |list: &[u32]| Value::Arr(list.iter().map(|pid| Value::Int(*pid as i64)).collect());
        if let Some(pid) = survivors.first() {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("worker pid {pid} for {account} survived SIGKILL"))),
                ("workers", pid_list(&pids)),
            ]);
        }
        object(vec![
            ("ok", Value::Bool(true)),
            ("job", job.map(|job| job.describe()).unwrap_or(Value::Null)),
            ("graceful", Value::Bool(graceful)),
            ("workers", pid_list(&pids)),
            ("untracked", pid_list(&terminated)),
        ])
    }

    /// `redeploy <account>`: close (no worker left, guaranteed) then open
    /// the fresh binary, under one claim. The reopen waits out the open
    /// pace and the server's hold on the dropped player (`RELOGIN_TICKS`).
    pub fn redeploy_job(self: &Arc<Self>, account: &str) -> Value {
        if account.is_empty() {
            return refusal("account is required");
        }
        let Some(_claim) = self.lifecycle.try_claim(account) else {
            return refusal(&format!("busy {account}: an open/close is in flight, retry"));
        };
        crate::brain_grace::note_operator_stop(account);
        let closed = self.close_claimed(account);
        if closed.get("ok").as_bool() != Some(true) {
            let error = closed.get("error").as_str().unwrap_or("close failed").to_string();
            if !error.starts_with("no session") {
                return refusal(&format!("redeploy {account}: close failed, not reopening: {error}"));
            }
        }
        let window = (RELOGIN_TICKS * lifescape_sdk::waits::tick_ms()) as f64 / 1000.0;
        let deadline = crate::jobs::now_secs() + window;
        loop {
            let reply = self.open_claimed(account, false);
            if reply.get("ok").as_bool() == Some(true) {
                crate::logln!("redeploy {account}: reopened");
                return object(vec![
                    ("ok", Value::Bool(true)),
                    ("job", reply.get("job").clone()),
                    ("closed", closed.get("workers").clone()),
                ]);
            }
            let error = reply.get("error").as_str().unwrap_or("open failed").to_string();
            match reopen_retry_secs(&error) {
                Some(wait) if crate::jobs::now_secs() + wait < deadline => {
                    std::thread::sleep(Duration::from_secs_f64(wait));
                }
                _ => return refusal(&format!("redeploy {account}: closed, reopen failed: {error}")),
            }
        }
    }

    /// `shutdown`: close every session in parallel (each one bounded),
    /// so one busy worker no longer delays all the others.
    pub(crate) fn close_all(&self) {
        let accounts: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        std::thread::scope(|scope| {
            for name in &accounts {
                scope.spawn(move || {
                    let _ = self.close_job(name);
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::reopen_retry_secs;

    /// Redeploy retries only the two known transient refusals.
    #[test]
    fn reopen_retries_pace_and_server_hold_only() {
        assert_eq!(reopen_retry_secs("admission:open-rate wait 0.6s"), Some(0.6));
        assert_eq!(reopen_retry_secs("admission:open-rate wait 0.0s"), Some(0.1), "never a busy loop");
        assert_eq!(
            reopen_retry_secs("login failed: login: login refused: 5 (already logged in)"),
            Some(2.0)
        );
        assert_eq!(reopen_retry_secs("worker died during open"), None);
        assert_eq!(reopen_retry_secs("live worker pid 15540 for ryan_park is not owned by this daemon"), None);
        assert_eq!(reopen_retry_secs("already open ryan_park"), None, "an open worker is a result, not a retry");
    }
}
