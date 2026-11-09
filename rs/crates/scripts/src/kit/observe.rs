//! Observe: per-run observation memory, owned by the bot and limited to
//! the current task. Regrow ETAs learned per product, confirmed stump
//! sightings, refusal strikes and gainless-stump backoff. Never shared
//! between bots, never persisted (D22): a fresh `Observe` starts every
//! run, and the maps die with it.

#![forbid(unsafe_code)]

use std::collections::HashMap;

/// Long sit-out for a condemned spot (structural bans, backed-off duds).
pub const STRUCTURAL_BAN_TICKS: u64 = 7_200;

/// Per-run observation memory: regrow delays per product, stump
/// sightings (spot -> tick), refusal strikes per spot and gainless-stump
/// counts per spot.
pub struct Observe {
    /// Observed regrow delay per product (server rate is per table).
    pub respawn: HashMap<String, u64>,
    /// Confirmed stump sightings (spot -> tick the stump was confirmed).
    pub stumped: HashMap<(i32, i32), u64>,
    /// Adjacent direct-refusal strikes per spot.
    pub refusals: HashMap<(i32, i32), u8>,
    /// Consecutive gainless stump visits per spot.
    pub dudded: HashMap<(i32, i32), u8>,
}

impl Observe {
    pub fn fresh() -> Observe {
        Observe { respawn: HashMap::new(), stumped: HashMap::new(), refusals: HashMap::new(), dudded: HashMap::new() }
    }

    /// Fold one observed regrow wait into the product ETA. Observations
    /// are REMAINING times (arrival mid-regrow reads short), so the ETA
    /// only ratchets up toward the true delay: overwriting with the
    /// latest collapsed a tier ETA and made every stump read due early.
    pub fn learn_respawn(&mut self, product: &str, observed: u64) {
        let wait = self.respawn.get(product).copied().unwrap_or(observed);
        self.respawn.insert(product.to_string(), wait.max(observed));
    }

    /// Timestamp a confirmed stump sighting.
    pub fn note_stump(&mut self, at: (i32, i32), now: u64) {
        self.stumped.insert(at, now);
    }

    /// A remembered stump whose regrow ETA passed reads as live: the bot
    /// wasn't there to see it regrow. Unknown spots (never seen stump)
    /// are NOT predicted — first sight stamps the clock, so at most one
    /// extra regrow window passes before the first probe.
    pub fn regrown_due(&mut self, now: u64, at: (i32, i32), wait: u64, stump_now: bool) -> bool {
        if !stump_now {
            self.stumped.remove(&at);
            return true;
        }
        let at_tick = *self.stumped.entry(at).or_insert(now);
        now.saturating_sub(at_tick) >= wait
    }

    /// Sit-out after a gainless outcome at `at`: gains prove the spot
    /// live (reset to the ETA); a gainless visit counts a failed
    /// prediction and backs off geometrically so a never-recovering spot
    /// stops shadowing the pool. Capped at the long sit-out.
    pub fn dud_ban(&mut self, at: (i32, i32), gained: u32, expected: u64) -> u64 {
        if gained > 0 {
            self.dudded.remove(&at);
            return expected;
        }
        let fails = self.dudded.get(&at).copied().unwrap_or(0).saturating_add(1);
        self.dudded.insert(at, fails);
        dud_ban_ticks(expected, fails)
    }
}

/// Sit-out for a gainless visit: the first rides the regrow ETA (a race
/// or a stale template costs one walk); repeats back off geometrically
/// so a never-recovering spot stops shadowing the pool. Capped at the
/// long sit-out; the starvation release still re-proves the oldest.
pub fn dud_ban_ticks(expected: u64, fails: u8) -> u64 {
    if fails <= 1 {
        return expected;
    }
    (expected << fails.saturating_sub(1).min(5)).min(STRUCTURAL_BAN_TICKS)
}

#[cfg(test)]
mod tests {
    use super::{dud_ban_ticks, Observe, STRUCTURAL_BAN_TICKS};

    /// Dud rule: a gainless visit rides the ETA once (a race or a stale
    /// template costs one walk), then backs off geometrically so a
    /// never-regrowing spot stops shadowing the pool; any gains prove it
    /// live and reset to the ETA.
    #[test]
    fn dud_ban_grace_then_backoff() {
        let mut observe = Observe::fresh();
        assert_eq!(observe.dud_ban((1, 1), 0, 200), 200, "first gainless visit rides the ETA");
        assert_eq!(observe.dud_ban((1, 1), 0, 200), 400, "repeat backs off");
        assert_eq!(observe.dud_ban((1, 1), 0, 200), 800, "still down backs off again");
        assert_eq!(observe.dud_ban((1, 1), 5, 200), 200, "gains prove live, ETA again");
        assert_eq!(observe.dud_ban((1, 1), 0, 200), 200, "grace again after reset");
        assert_eq!(dud_ban_ticks(400, 9), STRUCTURAL_BAN_TICKS, "capped at the long sit-out");
    }

    /// Learning rule: observations are REMAINING times (arrival mid-regrow
    /// reads short), so the ETA only ratchets up toward the true delay.
    #[test]
    fn respawn_learning_ratchets_up_never_down() {
        let mut observe = Observe::fresh();
        observe.learn_respawn("yew_logs", 185);
        assert_eq!(observe.respawn["yew_logs"], 185, "first observation sets the ETA");
        observe.learn_respawn("yew_logs", 10);
        assert_eq!(observe.respawn["yew_logs"], 185, "short remaining never collapses the ETA");
        observe.learn_respawn("yew_logs", 200);
        assert_eq!(observe.respawn["yew_logs"], 200, "longer observation recovers upward");
    }

    /// Due stumps read live (regrow ETA passed), fresh ones stay down,
    /// and a live tile clears the stamp.
    #[test]
    fn regrown_due_predicts_only_stamped_stumps() {
        let mut observe = Observe::fresh();
        assert!(observe.regrown_due(100, (1, 1), 30, false), "live tile reads live and clears");
        assert!(!observe.stumped.contains_key(&(1, 1)), "live tile clears the stamp");
        assert!(!observe.regrown_due(100, (1, 1), 30, true), "first sight stamps, not predicts");
        assert!(!observe.regrown_due(110, (1, 1), 30, true), "fresh stump stays down");
        assert!(observe.regrown_due(130, (1, 1), 30, true), "due stump reads live");
    }
}
