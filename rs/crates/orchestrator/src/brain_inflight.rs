//! Reports received but not folded yet (plan P10, 2026-11-17).
//!
//! A pushed report waits in its brain shard's inbox (or, inbox full, in
//! `JobInner::report_pending`) and is folded after its rates ingest (up
//! to 35 s under load). The idle detector of the launch pass ran in that
//! window and counted "no report: worker went idle without reporting":
//! 237 false failures in 10 h, three in a row banned a skill. The
//! detector now asks here first: a report in flight is the run's verdict,
//! never a missing one.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn table() -> &'static Mutex<HashMap<String, u32>> {
    static TABLE: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A report of `account` entered a brain inbox.
pub(crate) fn queued(account: &str) {
    *table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).entry(account.to_string()).or_insert(0) += 1;
}

/// A report of `account` was folded (or dropped).
pub(crate) fn folded(account: &str) {
    let mut guard = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = guard.get_mut(account) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            guard.remove(account);
        }
    }
}

/// True when a report of `account` is queued and not folded yet.
pub(crate) fn in_flight(account: &str) -> bool {
    table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).is_some_and(|count| *count > 0)
}

/// Pure verdict of the idle detector: the active node failed for want of
/// a report only when none is in flight (inbox) nor kept pending (inbox
/// full, `report_pending`).
pub fn missing_report(in_flight: bool, pending: bool) -> bool {
    !in_flight && !pending
}

#[cfg(test)]
mod tests {
    use super::{folded, in_flight, missing_report, queued};

    /// Plan P10 red test: an idle worker whose report waits in the inbox
    /// (or pending) is no "no report" failure; once folded, a later idle
    /// without report is.
    #[test]
    fn a_report_in_flight_is_never_a_missing_one() {
        assert!(!in_flight("p10_bot"));
        queued("p10_bot");
        assert!(in_flight("p10_bot"));
        assert!(!missing_report(in_flight("p10_bot"), false), "queued: the report decides");
        folded("p10_bot");
        assert!(!in_flight("p10_bot"));
        assert!(!missing_report(false, true), "inbox full, kept pending");
        assert!(missing_report(false, false), "nothing anywhere: a real crash");
        folded("p10_bot");
        assert!(!in_flight("p10_bot"), "never negative");
    }
}
