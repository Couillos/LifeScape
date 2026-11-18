//! Time and XP of a method read from its YIELD (plan rendements R3.3):
//! the planner never prices a level climb with its own constants (an
//! anvil press of 5 ticks, a 60-tick bank trip, 60 % of shots landing).
//! It reads one `WorkRate` per (method, level): actions, XP per skill
//! and signed flows per 6000 ticks of work, from
//! - the measured blend the brain reads (`orchestrator::rates_read`,
//!   handed in as a `RateReader`), or
//! - the game prior (`method_rate::method_prior`: server action ticks,
//!   odds at the level, bank walks), the XP derived from its units
//!   (`unit_xp::derived_xp`: units × XP of the dump), never measured.
//!
//! Everything a caller needs follows from it, universally:
//! - ticks per XP of a skill = 6000 / XP per 6000;
//! - XP per action = XP per 6000 / actions per 6000 (one ATTEMPT as the
//!   prior counts it: a press, a cast, a roll, a kill);
//! - units of a consumable burned per XP = |its flow| / XP per 6000
//!   (inputs, runes, ammunition, bait, thread).
//!
//! No rate = `None` (the caller skips the method, logged where it
//! decides): never a default figure (rule 7).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Ticks of the window every rate is expressed over.
pub const WINDOW_TICKS: f64 = 6000.0;
/// Flow causes a level climb burns (`sizing::tranche_consumption`):
/// recipe inputs, runes, ammunition, bait and supplies. Food, fees and
/// deaths are not the method's consumption.
pub const CONSUMED_CAUSES: [&str; 5] = ["input", "rune", "ammo", "bait", "supply"];
/// Priors memoised at most (cleared beyond).
pub const MEMO_MAX: usize = 8192;

/// One method's work per 6000 ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkRate {
    pub actions_6000: f64,
    /// (skill, XP per 6000) derived from the units.
    pub xp_6000: Vec<(String, f64)>,
    /// (debug item, cause, signed quantity per 6000).
    pub flows_6000: Vec<(String, String, f64)>,
}

/// A reader of rates: (method, level) → the rate a bot at that level
/// works at (`None`: no measure and no prior).
pub type RateReader<'a> = &'a dyn Fn(&str, u8) -> Option<WorkRate>;

impl WorkRate {
    /// XP of `skill` per 6000 ticks.
    pub fn xp_of(&self, skill: &str) -> f64 {
        self.xp_6000.iter().filter(|(name, _)| name == skill).map(|(_, gained)| *gained).sum()
    }

    /// Ticks one XP of `skill` takes (`None`: the method pays none).
    pub fn ticks_per_xp(&self, skill: &str) -> Option<f64> {
        let earned = self.xp_of(skill);
        (earned > 0.0).then(|| WINDOW_TICKS / earned)
    }

    /// XP of `skill` one action pays on average (failures included).
    pub fn xp_per_action(&self, skill: &str) -> Option<f64> {
        let earned = self.xp_of(skill);
        (earned > 0.0 && self.actions_6000 > 0.0).then(|| earned / self.actions_6000)
    }

    /// Units of each consumable (`CONSUMED_CAUSES`) burned per XP of
    /// `skill`, (debug item, units); empty when the method pays none.
    pub fn burned_per_xp(&self, skill: &str) -> Vec<(String, f64)> {
        let earned = self.xp_of(skill);
        if earned <= 0.0 {
            return Vec::new();
        }
        // Net of what the method itself takes in (oscar_cole 2026-11-17:
        // a fight burying the bones it loots read "missing bones"; arrows
        // picked back count only their loss).
        let mut out: Vec<(String, f64)> = Vec::new();
        for (item, cause, qty) in &self.flows_6000 {
            if *qty >= 0.0 || !CONSUMED_CAUSES.contains(&cause.as_str()) {
                continue;
            }
            match out.iter_mut().find(|(known, _)| known == item) {
                Some(known) => known.1 += -qty,
                None => out.push((item.clone(), -qty)),
            }
        }
        for (item, burnt) in out.iter_mut() {
            let taken: f64 = self.flows_6000.iter().filter(|(known, _, qty)| known == item && *qty > 0.0).map(|(_, _, qty)| *qty).sum();
            *burnt = (*burnt - taken).max(0.0) / earned;
        }
        out.retain(|(_, per_xp)| *per_xp > 0.0);
        out
    }
}

/// Combat style of a method as the reports carry it (`unit_xp`): ranged
/// and magic from the method, melee and the rest none (a melee climb's
/// skill rides in its node, not its method).
pub fn style_of(method: &str) -> &'static str {
    if method.starts_with("fight:ranged/") {
        "ranged"
    } else if method.starts_with("fight:magic/") {
        "magic"
    } else {
        ""
    }
}

/// The rate of a prior: its flows, and the XP its units pay at `level`.
pub fn from_prior(method: &str, level: u8, prior: &crate::method_rate::RatePrior) -> WorkRate {
    let units: Vec<(String, String, f64)> = prior.units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.clone(), *qty)).collect();
    WorkRate {
        actions_6000: prior.actions_per_6000,
        xp_6000: crate::unit_xp::derived_xp(method, style_of(method), &units, level.max(1)),
        flows_6000: prior.flows.iter().map(|(item, cause, qty)| (item.clone(), cause.to_string(), *qty)).collect(),
    }
}

/// The game prior's rate of `method` at `level` (no site, no tool: the
/// way's site nearest a bank, the base tool), memoised.
pub fn prior_rate(method: &str, level: u8) -> Option<WorkRate> {
    static MEMO: OnceLock<Mutex<HashMap<(String, u8), Option<WorkRate>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let key = (method.to_string(), level.max(1));
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return known.clone();
    }
    let query = crate::method_rate::PriorQuery { method: method.to_string(), level: level.max(1), tool: String::new(), site: None };
    let found = crate::method_rate::method_prior(&query).ok().map(|prior| from_prior(method, level, &prior));
    let mut guard = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() >= MEMO_MAX {
        guard.clear();
    }
    guard.insert(key, found.clone());
    found
}

#[cfg(test)]
#[path = "work_rate_tests.rs"]
mod tests;
