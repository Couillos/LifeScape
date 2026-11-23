#![forbid(unsafe_code)]
//! When a resting gear bid may be withdrawn as "not wanted by the gear
//! plan" (user 2026-11-22: « 80 % des transactions annulées »).
//!
//! Live, 6 h: 2 501 bids closed `gear_plan`. 1 818 were ARBITRAGE bids
//! (mean life 6 s: the arbitrage places a bid on a wearable, the next
//! gear pass finds the piece in no slot of its plan and cancels it, the
//! arbitrage re-places it) and 45 were stock bids: the gear plan owns
//! only the bids it placed itself (motive `gear`). The other 638 were
//! its own bids (581 re-placed within 30 min): the held wants of a pass
//! drop a slot on a transient (budget dip, signature move) and bring it
//! back minutes later.
//!
//! A gear bid is therefore withdrawn only after its piece has stayed
//! out of the plan for `STALE_GRACE_SECS` without interruption; one
//! pass that wants it again resets the clock.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// Wall seconds a gear bid's piece must stay unwanted before the bid
/// is withdrawn (a flip of a few passes never cancels anything).
pub const STALE_GRACE_SECS: f64 = 1800.0;

/// One resting bid the gear plan may judge: `(order id, piece name)`.
pub type Candidate = (i64, String);

/// `first_unwanted` is the account's memory (piece -> wall time it was
/// first seen unwanted). Returns the candidates to withdraw now: those
/// unwanted for at least `STALE_GRACE_SECS`. Pieces no longer
/// candidates (wanted again, bid gone) are forgotten.
pub fn due_for_withdrawal(first_unwanted: &mut HashMap<String, f64>, candidates: &[Candidate], now: f64) -> Vec<Candidate> {
    let present: HashSet<&str> = candidates.iter().map(|(_, name)| name.as_str()).collect();
    first_unwanted.retain(|name, _| present.contains(name.as_str()));
    let mut due_names: HashSet<String> = HashSet::new();
    for (_, name) in candidates {
        let since = *first_unwanted.entry(name.clone()).or_insert(now);
        if now - since >= STALE_GRACE_SECS {
            due_names.insert(name.clone());
        }
    }
    first_unwanted.retain(|name, _| !due_names.contains(name));
    candidates.iter().filter(|(_, name)| due_names.contains(name)).cloned().collect()
}

fn table() -> &'static Mutex<HashMap<String, HashMap<String, f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, HashMap<String, f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The account's candidates to withdraw this pass (memory kept per account).
pub fn due_for_account(account: &str, candidates: &[Candidate], now: f64) -> Vec<Candidate> {
    let mut accounts = table().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    due_for_withdrawal(accounts.entry(account.to_string()).or_default(), candidates, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(order: i64, name: &str) -> Candidate {
        (order, name.to_string())
    }

    /// The live flip: the piece leaves the plan for a few passes then comes
    /// back; nothing is ever withdrawn.
    #[test]
    fn a_flip_of_a_few_passes_cancels_nothing() {
        let mut memory = HashMap::new();
        let bid = [candidate(1, "adamant_kiteshield")];
        for pass in 0..5 {
            assert!(due_for_withdrawal(&mut memory, &bid, 1000.0 + 60.0 * pass as f64).is_empty());
        }
        assert!(due_for_withdrawal(&mut memory, &[], 1300.0).is_empty(), "wanted again: the clock resets");
        assert!(due_for_withdrawal(&mut memory, &bid, 1360.0).is_empty());
        assert!(due_for_withdrawal(&mut memory, &bid, 1360.0 + STALE_GRACE_SECS - 1.0).is_empty(), "the grace restarted at 1360");
    }

    #[test]
    fn a_piece_unwanted_for_the_whole_grace_is_withdrawn_once() {
        let mut memory = HashMap::new();
        let bid = [candidate(7, "steel_platebody")];
        assert!(due_for_withdrawal(&mut memory, &bid, 0.0).is_empty());
        assert_eq!(due_for_withdrawal(&mut memory, &bid, STALE_GRACE_SECS), vec![candidate(7, "steel_platebody")]);
        assert!(due_for_withdrawal(&mut memory, &bid, STALE_GRACE_SECS + 60.0).is_empty(), "the clock restarts after a withdrawal");
    }
}
