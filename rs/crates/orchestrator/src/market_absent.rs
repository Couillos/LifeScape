#![forbid(unsafe_code)]
//! Absent parties (user rule 2026-11-13: "if a party is disconnected when
//! an order fills, connect it and have it trade"). A matched deal only
//! convenes once both parties push a live position; a party without a
//! session used to keep the deal `matched` forever (live: deal 977,
//! seller t01 offline, "parties missing" every round).
//!
//! Each convene round: an absent party with no session is opened through
//! the daemon's own `open_job` (E1 admission and E10 open pace included),
//! one account at a time, off the convene thread (an open blocks up to
//! the open timeout). An account that cannot be opened (staff/private,
//! login refused) or that stays absent past `ABSENT_GRACE_TICKS` fails
//! its matched deals `no_show`: the honest side relists with priority,
//! the absent side's orders are cancelled (`fail_absent_party`).
//!
//! `plan` is pure: the caller hands it the absent set, the session test
//! and the exclusion list, so the state machine is pinned by unit tests.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use lifescape_json::Value;
use lifescape_market::DealStatus;

use crate::daemon::Orchestrator;
use crate::market_service::MarketService;

/// Bound on an absent party (server ticks at the live rate): ~3 min in
/// dev (60 ms), 30 min in prod (600 ms). Covers an open (login, first
/// snapshot) and a daemon restart's re-adoption with room to spare.
pub const ABSENT_GRACE_TICKS: u64 = 3000;

/// What the convene thread does for one absent account this round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbsentAction {
    /// Open this account's session (one at a time).
    Open(String),
    /// Give up: fail its matched deals, cancel its orders. Carries why.
    Fail(String, String),
}

/// Per-account memory across convene rounds.
#[derive(Default)]
pub struct AbsentTracker {
    /// Wall second each account was first seen absent.
    first_missing: HashMap<String, f64>,
    /// Hard open failures (never retried): account to the open error.
    open_failed: HashMap<String, String>,
    /// The account whose open runs right now, if any.
    in_flight: Option<String>,
    /// Finished opens, filled by the open thread, drained each round.
    results: Arc<Mutex<Vec<(String, Value)>>>,
}

/// Grace in wall seconds at the live tick rate.
pub fn grace_secs(tick_ms: u64) -> f64 {
    ABSENT_GRACE_TICKS as f64 * tick_ms as f64 / 1000.0
}

impl AbsentTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// One round of decisions. `absent` lists the parties of matched
    /// deals with no live position; `has_session` says whether a worker
    /// already runs for an account (then we only wait for its snapshot).
    pub fn plan(
        &mut self,
        absent: &[String],
        has_session: &dyn Fn(&str) -> bool,
        excluded: &BTreeSet<String>,
        now_wall: f64,
        tick_ms: u64,
    ) -> Vec<AbsentAction> {
        let still: HashSet<&String> = absent.iter().collect();
        self.first_missing.retain(|account, _| still.contains(account));
        self.open_failed.retain(|account, _| still.contains(account));
        let mut sorted: Vec<&String> = still.into_iter().collect();
        sorted.sort();
        let mut actions = Vec::new();
        for account in sorted {
            let first = *self.first_missing.entry(account.clone()).or_insert(now_wall);
            let opening = self.in_flight.as_deref() == Some(account.as_str());
            let verdict = if excluded.contains(&account.to_lowercase()) {
                Some("staff or private account, never opened automatically".to_string())
            } else if let Some(error) = self.open_failed.get(account) {
                Some(format!("open failed: {error}"))
            } else if !opening && now_wall - first > grace_secs(tick_ms) {
                Some(format!("still absent after {ABSENT_GRACE_TICKS} ticks"))
            } else {
                None
            };
            if let Some(why) = verdict {
                self.first_missing.remove(account);
                self.open_failed.remove(account);
                actions.push(AbsentAction::Fail(account.clone(), why));
                continue;
            }
            if self.in_flight.is_none() && !has_session(account) {
                self.in_flight = Some(account.clone());
                actions.push(AbsentAction::Open(account.clone()));
            }
        }
        actions
    }

    /// Outcome of one open (`open_job` reply). Admission pace refusals
    /// and an already running worker retry or wait; anything else is a
    /// hard failure the next `plan` turns into `Fail`.
    pub fn record_open(&mut self, account: &str, reply: &Value) {
        if self.in_flight.as_deref() == Some(account) {
            self.in_flight = None;
        }
        if reply.get("ok").as_bool() == Some(true) {
            return;
        }
        let error = reply.get("error").as_str().unwrap_or("open failed without a reason").to_string();
        if error.starts_with("admission:open-rate") || error.starts_with("already open") {
            return;
        }
        self.open_failed.insert(account.to_string(), error);
    }
}

/// Parties of matched deals with no live position this round.
pub fn absent_parties<Position>(service: &MarketService, parties: &HashMap<String, Position>) -> Vec<String> {
    let engine = service.lock_engine();
    let mut absent: BTreeSet<String> = BTreeSet::new();
    for deal in engine.deals.values().filter(|deal| deal.status == DealStatus::Matched) {
        for account in [&deal.buyer, &deal.seller] {
            if !parties.contains_key(account) {
                absent.insert(account.clone());
            }
        }
    }
    absent.into_iter().collect()
}

/// Convene-thread hook: drain finished opens, plan, act. Opens run on
/// their own thread (one at a time); failures fail the deals at once.
pub fn absent_round<Position>(
    tracker: &mut AbsentTracker,
    orch: &Arc<Orchestrator>,
    service: &MarketService,
    parties: &HashMap<String, Position>,
    now_wall: f64,
    tick_ms: u64,
) {
    let finished: Vec<(String, Value)> = tracker.results.lock().map(|mut done| done.drain(..).collect()).unwrap_or_default();
    for (account, reply) in finished {
        if reply.get("ok").as_bool() == Some(true) {
            crate::logln!("market-absent: {account} opened for its matched deals");
        } else {
            crate::logln!("market-absent: open {account} refused ({})", reply.get("error").as_str().unwrap_or("?"));
        }
        tracker.record_open(&account, &reply);
    }
    let absent = absent_parties(service, parties);
    if absent.is_empty() && tracker.first_missing.is_empty() {
        return;
    }
    let sessions: HashSet<String> = orch.jobs.lock().map(|jobs| jobs.keys().cloned().collect()).unwrap_or_default();
    let has_session = |account: &str| sessions.contains(account) && orch.proc_alive(account);
    let excluded = crate::economy_guard::excluded_accounts();
    for action in tracker.plan(&absent, &has_session, &excluded, now_wall, tick_ms) {
        match action {
            AbsentAction::Open(account) => {
                crate::logln!("market-absent: opening {account} (party of a matched deal, no session)");
                let opener = orch.clone();
                let results = tracker.results.clone();
                let spawned = std::thread::Builder::new().name("market-open".to_string()).spawn(move || {
                    let reply = opener.open_job(&account, false);
                    if let Ok(mut done) = results.lock() {
                        done.push((account, reply));
                    }
                });
                if let Err(error) = spawned {
                    crate::logln!("market-absent: open thread refused ({error})");
                    tracker.in_flight = None;
                }
            }
            AbsentAction::Fail(account, why) => {
                crate::logln!("market-absent: {account} cannot trade ({why}), failing its matched deals");
                service.fail_absent_party(&account);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{grace_secs, AbsentAction, AbsentTracker};
    use lifescape_json::{object, str_value, Value};
    use std::collections::BTreeSet;

    fn excluded() -> BTreeSet<String> {
        ["admin", "bite"].iter().map(|name| name.to_string()).collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    /// Absent without a session: opened, one account at a time; the next
    /// one waits until the first open reports.
    #[test]
    fn opens_one_absent_account_at_a_time() {
        let mut tracker = AbsentTracker::new();
        let none = |_: &str| false;
        let first = tracker.plan(&names(&["t02", "t01"]), &none, &excluded(), 100.0, 60);
        assert_eq!(first, vec![AbsentAction::Open("t01".to_string())], "sorted, single open");
        let busy = tracker.plan(&names(&["t02", "t01"]), &none, &excluded(), 110.0, 60);
        assert!(busy.is_empty(), "no parallel open: {busy:?}");
        tracker.record_open("t01", &object(vec![("ok", Value::Bool(true))]));
        let opened = |account: &str| account == "t01";
        let next = tracker.plan(&names(&["t02", "t01"]), &opened, &excluded(), 120.0, 60);
        assert_eq!(next, vec![AbsentAction::Open("t02".to_string())], "t01 has its session, t02 next");
    }

    /// Staff and private accounts are never opened: their deals fail.
    #[test]
    fn excluded_accounts_fail_without_open() {
        let mut tracker = AbsentTracker::new();
        let actions = tracker.plan(&names(&["bite"]), &|_| false, &excluded(), 100.0, 60);
        assert!(matches!(actions.as_slice(), [AbsentAction::Fail(account, _)] if account == "bite"), "{actions:?}");
    }

    /// Admission pace refusals retry; a hard open failure fails the deals.
    #[test]
    fn open_errors_retry_or_fail() {
        let mut tracker = AbsentTracker::new();
        let none = |_: &str| false;
        tracker.plan(&names(&["t01"]), &none, &excluded(), 100.0, 60);
        tracker.record_open("t01", &object(vec![("ok", Value::Bool(false)), ("error", str_value("admission:open-rate wait 1.2s"))]));
        let retry = tracker.plan(&names(&["t01"]), &none, &excluded(), 110.0, 60);
        assert_eq!(retry, vec![AbsentAction::Open("t01".to_string())], "pace refusal retries");
        tracker.record_open("t01", &object(vec![("ok", Value::Bool(false)), ("error", str_value("login refused"))]));
        let failed = tracker.plan(&names(&["t01"]), &none, &excluded(), 120.0, 60);
        assert!(matches!(failed.as_slice(), [AbsentAction::Fail(account, why)] if account == "t01" && why.contains("login refused")), "{failed:?}");
    }

    /// A session that never yields a position fails past the grace; a
    /// party that shows up is forgotten (its timer restarts next time).
    #[test]
    fn grace_bounds_a_silent_session() {
        let mut tracker = AbsentTracker::new();
        let running = |_: &str| true;
        assert!(tracker.plan(&names(&["t01"]), &running, &excluded(), 100.0, 60).is_empty(), "session up, wait");
        let within = 100.0 + grace_secs(60) - 1.0;
        assert!(tracker.plan(&names(&["t01"]), &running, &excluded(), within, 60).is_empty(), "inside the grace");
        assert!(tracker.plan(&[], &running, &excluded(), within, 60).is_empty(), "showed up");
        let late = within + grace_secs(60) - 1.0;
        assert!(tracker.plan(&names(&["t01"]), &running, &excluded(), late, 60).is_empty(), "timer restarted");
        let past = late + grace_secs(60) + 1.0;
        let failed = tracker.plan(&names(&["t01"]), &running, &excluded(), past, 60);
        assert!(matches!(failed.as_slice(), [AbsentAction::Fail(account, _)] if account == "t01"), "{failed:?}");
    }

    /// An open still running is never cut by the grace.
    #[test]
    fn grace_waits_for_an_open_in_flight() {
        let mut tracker = AbsentTracker::new();
        tracker.plan(&names(&["t01"]), &|_| false, &excluded(), 100.0, 60);
        let past = 100.0 + grace_secs(60) + 5.0;
        assert!(tracker.plan(&names(&["t01"]), &|_| false, &excluded(), past, 60).is_empty(), "open in flight");
    }
}
