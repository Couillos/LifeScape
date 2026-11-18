#![forbid(unsafe_code)]
//! Fleet levels for the gear plan's "obtainable now" (user 2026-11-18:
//! nathan_t's 529 mithril kiteshields unsold while 40 fighters bid on
//! rune and adamant pieces nobody sells and no fleet smith can make).
//!
//! Every gear pass records its bot's live levels; a piece's production
//! way counts when the best recorded fleet level of its skill reaches the
//! way's level (`gear_plan::obtainable_now`). Staff and personal accounts
//! never count (`economy_guard::excluded_accounts`), records older than
//! `FLEET_LEVELS_MAX_AGE_SECS` drop out. Right after a daemon start the
//! table fills within one gear period: the plan waits until it holds
//! `WARM_PCT` of the fleet, never planning from a partial fleet (the
//! held plan would keep that view for a whole replan period).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A record older than this (wall seconds) is a bot that left.
pub const FLEET_LEVELS_MAX_AGE_SECS: f64 = 3600.0;
/// Share of the fleet recorded before the plan trusts the table.
pub const WARM_PCT: usize = 75;

/// Recorded levels: account → (wall seconds, skill levels).
#[derive(Default)]
pub struct FleetLevels {
    records: HashMap<String, (f64, Vec<(String, i64)>)>,
}

impl FleetLevels {
    pub fn record(&mut self, account: &str, now: f64, levels: &[(&str, i64)]) {
        let owned = levels.iter().map(|(skill, level)| (skill.to_string(), *level)).collect();
        self.records.insert(account.to_lowercase(), (now, owned));
    }

    fn fresh(&self, now: f64) -> impl Iterator<Item = &Vec<(String, i64)>> {
        self.records.values().filter(move |(at, _)| now - at <= FLEET_LEVELS_MAX_AGE_SECS).map(|(_, levels)| levels)
    }

    /// Best fresh level of `skill` across the fleet (1 when none).
    pub fn best(&self, skill: &str, now: f64) -> i64 {
        self.fresh(now).filter_map(|levels| levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level)).max().unwrap_or(1)
    }

    /// True once `WARM_PCT` of a `fleet_size` fleet is recorded.
    pub fn warm(&self, fleet_size: usize, now: f64) -> bool {
        self.fresh(now).count() * 100 >= fleet_size * WARM_PCT
    }
}

fn table() -> &'static Mutex<FleetLevels> {
    static TABLE: OnceLock<Mutex<FleetLevels>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(FleetLevels::default()))
}

/// Records a fleet bot's levels (staff and personal accounts skipped).
pub(crate) fn record(account: &str, now: f64, levels: &[(&str, i64)]) {
    if crate::economy_guard::excluded_accounts().contains(&account.to_lowercase()) {
        return;
    }
    table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).record(account, now, levels);
}

/// Best fleet level of `skill` now.
pub(crate) fn best(skill: &str, now: f64) -> i64 {
    table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).best(skill, now)
}

/// Best fleet level of `skill` at the wall clock.
pub(crate) fn best_now(skill: &str) -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0);
    best(skill, now)
}

/// True once the table holds enough of the running fleet.
pub(crate) fn warm(now: f64) -> bool {
    let fleet_size = crate::brain_ambition::fleet_size();
    table().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).warm(fleet_size, now)
}

#[cfg(test)]
mod tests {
    use super::FleetLevels;

    /// The best fresh level counts; a bot gone for an hour does not; the
    /// table is warm once 75 % of the fleet is recorded.
    #[test]
    fn best_fresh_level_and_warmth() {
        let mut fleet = FleetLevels::default();
        fleet.record("nathan_t", 5000.0, &[("smithing", 68), ("crafting", 30)]);
        fleet.record("old_smith", 0.0, &[("smithing", 90)]);
        fleet.record("tim_thorn", 5000.0, &[("smithing", 46)]);
        assert_eq!(fleet.best("smithing", 5000.0), 68, "the 90 recorded over an hour ago left");
        assert_eq!(fleet.best("magic", 5000.0), 1, "no record: level 1");
        assert!(!fleet.warm(4, 5000.0), "2 fresh of 4");
        fleet.record("felix_rook", 5000.0, &[("smithing", 1)]);
        assert!(fleet.warm(4, 5000.0), "3 fresh of 4");
        assert!(fleet.warm(0, 5000.0), "no fleet known yet");
    }
}
