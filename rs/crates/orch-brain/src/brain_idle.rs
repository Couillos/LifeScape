#![forbid(unsafe_code)]
//! Idle-bot detector (user rule 2026-11-14 "aucun bot inactif", no
//! silent loop). A brain-run worker idle (no script) for
//! `IDLE_ALERT_SECS` while the operator neither paused nor held it, and
//! still connected, raises ONE log line `brain <acct>: idle <N>s without
//! a script: <reasons>` — an `idle_bot` row on the errors page — then
//! again every `IDLE_REPEAT_SECS` while it lasts. The reasons are the
//! account's last brain decisions (live 2026-11-14, ryan_mason: 30+ min
//! of `blocked at resolve` lines and no visible error at all).
//!
//! Pure core (`IdleWatch`, `remember_line` filtering) unit-tested; the
//! supervisor feeds it every sweep without blocking (`watch`, try-lock).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use crate::jobs::Job;

/// Idle time before the first alert of a spell.
pub const IDLE_ALERT_SECS: f64 = 300.0;
/// Alert again while the same spell lasts.
pub const IDLE_REPEAT_SECS: f64 = 900.0;
/// Brain decisions kept per account as the alert's reasons.
const REASON_LINES: usize = 3;
/// Longest reason line kept.
const REASON_CHARS: usize = 240;
/// Marker of the alert line (parsed by `errors_model::from_brain_line`).
pub use crate::log_markers::ALERT_MARKER;

/// What the supervisor read of a worker this sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// A script (node, filler, market delivery, rescue) runs.
    Working,
    /// No script, connected, neither paused nor held.
    Idle,
    /// Paused or held by the operator, or disconnected: never alerted.
    Excused,
}

/// One account's idle spell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IdleWatch {
    since: Option<f64>,
    alerted: Option<f64>,
}

impl IdleWatch {
    /// Fold one sweep; `Some(seconds idle)` when an alert is due.
    pub fn observe(&mut self, state: WorkerState, now: f64) -> Option<u64> {
        if state != WorkerState::Idle {
            *self = IdleWatch::default();
            return None;
        }
        let since = *self.since.get_or_insert(now);
        let idle = now - since;
        let due = idle >= IDLE_ALERT_SECS && self.alerted.is_none_or(|at| now - at >= IDLE_REPEAT_SECS);
        if !due {
            return None;
        }
        self.alerted = Some(now);
        Some(idle as u64)
    }
}

/// Worker state from the job mirror; `None` when the lock is contended
/// (the next sweep reads it).
fn worker_state(job: &Job) -> Option<WorkerState> {
    let inner = job.inner.try_lock().ok()?;
    Some(if inner.paused || inner.held || inner.status == crate::job_state::Status::Disconnected {
        WorkerState::Excused
    } else if inner.phase == crate::job_state::Phase::Idle {
        WorkerState::Idle
    } else {
        WorkerState::Working
    })
}

fn watches() -> &'static Mutex<HashMap<String, IdleWatch>> {
    static TABLE: OnceLock<Mutex<HashMap<String, IdleWatch>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

fn reasons() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// Brain lines that say nothing about why a bot does not work: market
/// bookkeeping, back-offs, the alert itself.
fn noise(rest: &str) -> bool {
    const NOISE: [&str; 12] = [
        "listed ",
        "listing ",
        "holding ",
        "wish ",
        "gear bid",
        "standing bid",
        "backing off",
        "inner contended",
        "supervise_once",
        "rates ",
        "make: ",
        "idle ",
    ];
    NOISE.iter().any(|prefix| rest.starts_with(prefix)) || rest.contains(" listings adopted")
}

/// `(account, decision)` of a brain line worth keeping as a reason.
pub fn reason_of(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("brain ")?;
    let (account, decision) = rest.split_once(": ")?;
    if account.is_empty() || account.contains(' ') || noise(decision) {
        return None;
    }
    Some((account, decision))
}

/// Hook of every daemon log line (`logln!`): keeps the account's last
/// brain decisions.
pub fn remember_line(line: &str) {
    let Some((account, decision)) = reason_of(line) else { return };
    let decision: String = decision.chars().take(REASON_CHARS).collect();
    let mut table = reasons().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let kept = table.entry(account.to_string()).or_default();
    if kept.back() == Some(&decision) {
        return;
    }
    kept.push_back(decision);
    while kept.len() > REASON_LINES {
        kept.pop_front();
    }
}

/// The account's last brain decisions, oldest first (`?` when none).
pub fn reasons_of(account: &str) -> String {
    let table = reasons().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match table.get(account) {
        Some(kept) if !kept.is_empty() => kept.iter().cloned().collect::<Vec<_>>().join(" | "),
        _ => "no brain decision logged since the daemon started".to_string(),
    }
}

/// Supervisor hook (every sweep, never blocks): one alert line per due
/// idle spell of a brain-run account (a life file: manual bots excluded).
pub fn watch(job: &Job) {
    let Some(state) = worker_state(job) else { return };
    let now = crate::jobs::now_secs();
    let due = {
        let mut table = watches().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        table.entry(job.account.clone()).or_default().observe(state, now)
    };
    let Some(idle) = due else { return };
    if !crate::paths::life_path(&job.account).exists() {
        return;
    }
    crate::logln!("brain {}: idle {idle}{ALERT_MARKER}{}", job.account, reasons_of(&job.account));
}

#[cfg(test)]
mod tests {
    use super::{reason_of, IdleWatch, WorkerState, IDLE_ALERT_SECS, IDLE_REPEAT_SECS};

    /// Five idle minutes raise one alert, repeated every 15 minutes of
    /// the same spell; a script or an operator pause ends the spell.
    #[test]
    fn an_idle_spell_alerts_once_then_repeats() {
        let mut watch = IdleWatch::default();
        assert_eq!(watch.observe(WorkerState::Idle, 1000.0), None);
        assert_eq!(watch.observe(WorkerState::Idle, 1000.0 + IDLE_ALERT_SECS - 1.0), None, "not yet");
        assert_eq!(watch.observe(WorkerState::Idle, 1000.0 + IDLE_ALERT_SECS), Some(IDLE_ALERT_SECS as u64));
        assert_eq!(watch.observe(WorkerState::Idle, 1000.0 + IDLE_ALERT_SECS + 60.0), None, "one alert per spell window");
        let again = 1000.0 + IDLE_ALERT_SECS + IDLE_REPEAT_SECS;
        assert_eq!(watch.observe(WorkerState::Idle, again), Some((IDLE_ALERT_SECS + IDLE_REPEAT_SECS) as u64), "still idle: repeated");
        assert_eq!(watch.observe(WorkerState::Working, again + 1.0), None);
        assert_eq!(watch.observe(WorkerState::Idle, again + 2.0), None, "a fresh spell starts over");
        let mut paused = IdleWatch::default();
        for step in 0..20 {
            assert_eq!(paused.observe(WorkerState::Excused, f64::from(step) * 60.0), None, "operator pause: never alerted");
        }
    }

    /// The reasons are brain decisions; market bookkeeping and the alert
    /// itself are not.
    #[test]
    fn reasons_skip_bookkeeping_lines() {
        assert_eq!(
            reason_of("brain ryan_mason: n20 blocked at resolve (missing:combat:43) -> pending"),
            Some(("ryan_mason", "n20 blocked at resolve (missing:combat:43) -> pending"))
        );
        assert_eq!(reason_of("brain ryan_mason: listed 2/2 bodyrune @ 1 (reserve 1, found)"), None);
        assert_eq!(reason_of("brain ryan_mason: 10 listings adopted by demand pricing (0 raised to the reserve, 0 lowered to today's opening), 0 withdrawn"), None);
        assert_eq!(reason_of("brain ryan_mason: idle 300s without a script: x"), None);
        assert_eq!(reason_of("job ryan_mason: task_report"), None);
    }
}
