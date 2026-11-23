#![forbid(unsafe_code)]
//! Chained launch passes (user rule 2026-11-14 "aucun bot inactif").
//!
//! Most launch passes make ONE decision and return: a node closed
//! without a launch (already held, already reached), a block filed as a
//! sub-task, a market bid parked, a gear step withdrawn. The next
//! decision used to wait for the next plain tick (live, 60 bots: 1.5–2
//! minutes idle each time, owen_wood/lily_brook/jim_ellis after a gear
//! step held its piece, ryan_mason one node per ~1.5 min). A pass that
//! changed the life (saved it) without launching anything is followed at
//! once by another, up to `CHAINED_PASSES`; a launch, a pass that
//! changed nothing or a worker that left idle (checked first by every
//! pass) ends the chain.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::fleet::Fleet;
use crate::jobs::Job;

/// Passes per brain tick at most (one decision each).
pub const CHAINED_PASSES: usize = 6;

/// Per-account counters: (life saves, launches).
fn counters() -> &'static Mutex<HashMap<String, (u64, u64)>> {
    static TABLE: OnceLock<Mutex<HashMap<String, (u64, u64)>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// A life file of `account` was written by the brain.
pub fn note_saved(account: &str) {
    let mut table = counters().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = table.entry(account.to_string()).or_insert((0, 0));
    entry.0 = entry.0.wrapping_add(1);
}

/// A script (node or filler) was queued for `account`.
pub fn note_launched(account: &str) {
    let mut table = counters().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = table.entry(account.to_string()).or_insert((0, 0));
    entry.1 = entry.1.wrapping_add(1);
}

fn generation(account: &str) -> (u64, u64) {
    counters().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(account).copied().unwrap_or((0, 0))
}

/// True when a pass that moved the counters from `before` to `after`
/// calls for another one: it saved the life and launched nothing.
pub fn chain_again(before: (u64, u64), after: (u64, u64)) -> bool {
    after.1 == before.1 && after.0 != before.0
}

/// One brain tick: launch passes chained while each one decides
/// something without launching.
pub fn run(orch: &Arc<dyn Fleet>, job: &Arc<Job>) {
    for _ in 0..CHAINED_PASSES {
        let before = generation(&job.account);
        crate::brain_launch::maybe_launch(orch, job);
        if !chain_again(before, generation(&job.account)) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{chain_again, note_launched, note_saved, generation};

    /// A pass that saved without launching chains; a launch or a pass
    /// that changed nothing ends the chain.
    #[test]
    fn only_a_decision_without_launch_chains() {
        let account = "chain_probe";
        let start = generation(account);
        note_saved(account);
        assert!(chain_again(start, generation(account)), "saved, nothing launched: next pass at once");
        let saved = generation(account);
        assert!(!chain_again(saved, generation(account)), "nothing changed: the tick ends");
        note_saved(account);
        note_launched(account);
        assert!(!chain_again(saved, generation(account)), "a launch ends the chain");
    }
}
