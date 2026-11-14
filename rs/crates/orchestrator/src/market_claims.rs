#![forbid(unsafe_code)]
//! Public order board claims (2026-11-14, user requirement: account for
//! the supply already in flight from other producers answering the same
//! object). A producer that takes a lot (`bid:<item>` goal) posts it on
//! the board, like a crafting order taken in a real game: the board is
//! PUBLIC (every bot reads the claimed units per object, never who nor
//! anything else of the claimer), so D22 holds — no hidden memory, the
//! same fact a player reads on the order board. Claims live in memory
//! (a restart forgets them; the books stay), expire after twice the lot's
//! estimated duration, and end when the claimer draws its next goal.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Shortest claim (wall seconds).
pub const MIN_CLAIM_SECS: i64 = 600;
/// Longest claim (wall seconds).
pub const MAX_CLAIM_SECS: i64 = 6 * 3600;

#[derive(Debug, Clone, PartialEq)]
struct Claim {
    account: String,
    item: String,
    units: i64,
    expires_secs: i64,
}

fn board() -> &'static Mutex<Vec<Claim>> {
    static BOARD: OnceLock<Mutex<Vec<Claim>>> = OnceLock::new();
    BOARD.get_or_init(|| Mutex::new(Vec::new()))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Wall seconds a claim of a lot estimated at `ticks` lasts.
pub fn claim_secs(ticks: f64, tick_ms: u64) -> i64 {
    let secs = (ticks.max(0.0) * 2.0 * tick_ms.max(1) as f64 / 1000.0).ceil() as i64;
    secs.clamp(MIN_CLAIM_SECS, MAX_CLAIM_SECS)
}

/// `account` takes `units` of `item` (its previous claims end).
pub fn claim(account: &str, item: &str, units: i64, secs: i64) {
    claim_at(account, item, units, now_secs() + secs);
}

fn claim_at(account: &str, item: &str, units: i64, expires_secs: i64) {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| !claim.account.eq_ignore_ascii_case(account));
    if units > 0 {
        table.push(Claim { account: account.to_string(), item: item.to_string(), units, expires_secs });
    }
}

/// The claims of `account` end (it draws its next goal).
pub fn release(account: &str) {
    board().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).retain(|claim| !claim.account.eq_ignore_ascii_case(account));
}

/// Units per object claimed by every other account (live ones only).
pub fn claimed_by_others(account: &str) -> HashMap<String, i64> {
    claimed_at(account, now_secs())
}

fn claimed_at(account: &str, now: i64) -> HashMap<String, i64> {
    let mut table = board().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    table.retain(|claim| claim.expires_secs > now);
    let mut out: HashMap<String, i64> = HashMap::new();
    for claim in table.iter().filter(|claim| !claim.account.eq_ignore_ascii_case(account)) {
        *out.entry(claim.item.clone()).or_default() += claim.units;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{claim_at, claim_secs, claimed_at, release, MAX_CLAIM_SECS, MIN_CLAIM_SECS};

    /// Another producer's lot is supply in flight; one's own is not; a
    /// new claim replaces the old, release and expiry end it.
    #[test]
    fn claims_are_public_supply_in_flight() {
        claim_at("claims_smith_a", "claims_iron_full_helm", 2, 1_000);
        claim_at("claims_smith_b", "claims_iron_full_helm", 3, 2_000);
        assert_eq!(claimed_at("claims_smith_a", 500).get("claims_iron_full_helm"), Some(&3));
        assert_eq!(claimed_at("claims_other", 500).get("claims_iron_full_helm"), Some(&5));
        assert_eq!(claimed_at("claims_other", 1_500).get("claims_iron_full_helm"), Some(&3), "the first expired");
        claim_at("claims_smith_b", "claims_iron_dagger", 4, 2_000);
        assert_eq!(claimed_at("claims_other", 1_500).get("claims_iron_full_helm"), None, "replaced");
        release("claims_smith_b");
        assert_eq!(claimed_at("claims_other", 1_500).get("claims_iron_dagger"), None, "released");
        assert_eq!(claim_secs(100.0, 60), MIN_CLAIM_SECS);
        assert_eq!(claim_secs(1e9, 600), MAX_CLAIM_SECS);
        assert_eq!(claim_secs(2_000.0, 600), 2_400);
    }
}
