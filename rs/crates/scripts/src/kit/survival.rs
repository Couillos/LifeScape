//! Survival: eating, death tally and the safe-point predicate. Every
//! archetype needs it (Wilderness open to all): eat under a threshold,
//! count consecutive deaths, resume after respawn, and expose the single
//! place that knows what "safe to pause" means for the market pause and
//! the end of engagement (L2 wires the callers).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::skill_common::hitpoints;
use crate::ScriptError;

/// Eat when hp falls to this fraction of the tracked max (fight loop
/// owns the max; beginners without food bank before hunting instead).
pub const EAT_FRACTION: f64 = 0.55;
/// Food carried after a bank restock.
pub const FOOD_TARGET: usize = 10;
/// Ticks waited flat after a death before resuming (respawn + resync).
pub const DEATH_WAIT_TICKS: u64 = 40;

/// True when hp sits at or under the eat line.
pub fn should_eat(hp: u8, max_hp: u8, fraction: f64) -> bool {
    f64::from(hp) <= f64::from(max_hp) * fraction
}

/// Eat the first edible thing in the pack. True when something was eaten.
pub fn eat(bot: &mut Bot) -> Result<bool, ScriptError> {
    let found = bot.backpack().1.into_iter().find(|(_, (held, _))| {
        (bot.tables.obj_name)(*held).is_some_and(|name| lifescape_gamedata::food::heal(&name) > 0)
    });
    let Some((slot, (held, _))) = found else {
        return Ok(false);
    };
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    bot.op_held(1, held, slot, pack_com)?;
    bot.pump(3)?;
    Ok(true)
}

/// Edible count in the pack.
pub fn food_count(bot: &Bot) -> u32 {
    bot.backpack()
        .1
        .into_iter()
        .filter(|(_, (held, _))| {
            (bot.tables.obj_name)(*held).is_some_and(|name| lifescape_gamedata::food::heal(&name) > 0)
        })
        .map(|(_, (_, count))| count)
        .sum()
}

/// Withdraw the best banked food up to the target. Returns food carried.
pub fn withdraw_food(bot: &mut Bot) -> Result<u32, ScriptError> {
    let banked: Vec<String> =
        bot.bank_items().keys().filter_map(|id| (bot.tables.obj_name)(*id)).collect();
    let refs: Vec<&str> = banked.iter().map(String::as_str).collect();
    let Some((chosen, _)) = lifescape_gamedata::food::best_food(&refs) else {
        return Ok(food_count(bot));
    };
    let food_id = crate::skill_common::obj_id(bot, &chosen)?;
    let need = (FOOD_TARGET as u32).saturating_sub(food_count(bot));
    if need > 0 {
        let _ = bot.bank_withdraw(food_id, need, 50);
    }
    Ok(food_count(bot))
}

/// True when the bank itself holds edible food: a foodless pack is only
/// worth a bank trip when the bank can restock it. A broke beginner
/// (empty bank) otherwise ping-pongs bank<->field after every kill and
/// dies crossing the map instead of hunting.
pub fn bank_stocks_food(bot: &Bot) -> bool {
    let banked: Vec<String> =
        bot.bank_items().keys().filter_map(|id| (bot.tables.obj_name)(*id)).collect();
    let refs: Vec<&str> = banked.iter().map(String::as_str).collect();
    lifescape_gamedata::food::best_food(&refs).is_some()
}

/// Run-local death tally: consecutive deaths (a kill or a bank trip is
/// progress and resets the streak) plus the killer spots excluded from
/// every later re-pick. Local to one run, never global, never persisted:
/// a spot that killed once stays out for the run.
pub struct DeathTally {
    deaths: u32,
    excluded: Vec<&'static str>,
}

impl DeathTally {
    pub fn fresh() -> DeathTally {
        DeathTally { deaths: 0, excluded: Vec::new() }
    }

    pub fn excludes(&self, spot_key: &str) -> bool {
        self.excluded.iter().any(|key| *key == spot_key)
    }

    /// Raw consecutive-death count (the typed error quotes it).
    pub fn deaths(&self) -> u32 {
        self.deaths
    }

    /// Killer spots excluded from every later re-pick.
    pub fn excluded(&self) -> &[&'static str] {
        &self.excluded
    }

    /// Exclude a spot without a death (auto-mode flank fallback).
    pub fn exclude(&mut self, spot_key: &'static str) {
        if !self.excludes(spot_key) {
            self.excluded.push(spot_key);
        }
    }

    pub fn fatal(&self) -> bool {
        self.deaths >= 2
    }

    /// A kill or a bank trip is progress: the next death starts a new
    /// streak. Exclusions stay: a spot that killed once stays out.
    pub fn note_progress(&mut self) {
        self.deaths = 0;
    }

    pub fn note_death(&mut self, spot_key: &'static str) {
        self.deaths += 1;
        if !self.excludes(spot_key) {
            self.excluded.push(spot_key);
        }
    }
}

/// Second consecutive death with no kill or bank trip between: loud and
/// typed instead of walking back a third time.
pub fn danger_error(spot_key: &str, deaths: u32) -> ScriptError {
    ScriptError::Sdk(lifescape_sdk::Error::Unreachable(format!(
        "spot {spot_key} too dangerous after {deaths} deaths, re-gear or pick easier foes"
    )))
}

/// Death resume: bounded wait for the respawn, then resync the loop
/// watches (hp flatlined, message index consumed). Never billed to a
/// stall budget by the caller. This is the `death_resume` the pause and
/// engagement code (L2) reuses; `skill_loop` wraps it for the driver.
pub fn death_resume(bot: &mut Bot, wait_ticks: u64, last_hp: &mut u8, seen: &mut usize) -> Result<(), ScriptError> {
    bot.log("died, respawning");
    bot.intend("died, respawning");
    let target = bot.world.ticks + wait_ticks;
    while bot.world.ticks < target && hitpoints(bot) == 0 {
        bot.pump(5)?;
    }
    *last_hp = hitpoints(bot);
    *seen = bot.world.messages.len();
    Ok(())
}

/// Safe-point predicate for the market pause and the end of engagement:
/// alive with no modal open (no dialogue, no trade screen). The combat
/// check (no target) lives with the fight loop, which owns its target;
/// the pause gate ANDs this with "not fighting" at the call site (L2).
pub fn at_safe_point(bot: &Bot) -> bool {
    hitpoints(bot) > 0 && bot.world.modal.is_none()
}

#[cfg(test)]
mod tests {
    use super::{at_safe_point, should_eat, DeathTally, EAT_FRACTION, FOOD_TARGET};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Eat line: at or under the fraction eats, above holds.
    #[test]
    fn eat_line_holds_above_falls_at_or_under() {
        assert!(should_eat(5, 10, 0.55), "5 of 10 at 0.55 eats");
        assert!(should_eat(6, 10, 0.6), "6 of 10 at 0.6 eats");
        assert!(!should_eat(6, 10, EAT_FRACTION), "6 of 10 above 0.55 holds");
        assert!(!should_eat(10, 10, EAT_FRACTION), "full never eats");
    }

    /// Tally: progress resets the streak, exclusions stay, two in a row
    /// without progress is fatal.
    #[test]
    fn tally_resets_on_progress_never_forgets_killers() {
        let mut tally = DeathTally::fresh();
        assert!(!tally.fatal());
        tally.note_death("goblins");
        assert!(tally.excludes("goblins"));
        assert!(!tally.fatal(), "one death is not fatal");
        tally.note_progress();
        tally.note_death("goblins");
        assert!(tally.excludes("goblins"), "exclusion survives progress");
        assert!(!tally.fatal(), "progress broke the streak");
        tally.note_death("cows");
        assert!(tally.fatal(), "two consecutive deaths are fatal");
    }

    /// Safe point: alive with no modal; dead or mid-dialogue never is.
    #[test]
    fn safe_point_needs_hp_and_no_modal() {        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        assert!(at_safe_point(&bot), "alive with no modal is safe");
        bot.world.stats.insert(3, (0, 0));
        assert!(!at_safe_point(&bot), "dead is never safe");
        assert_eq!(FOOD_TARGET, 10, "restock target");
    }

    /// A foodless pack forces a bank trip only when the bank can restock:
    /// broke beginners stay on the field instead of ping-ponging.
    #[test]
    fn bank_trip_needs_bank_food() {
        use super::bank_stocks_food;
        use crate::tables_live::attach_live_tables;
        use lifescape_sdk::Bot;
        let mut empty = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut empty);
        assert!(!bank_stocks_food(&empty), "no bank open: no trip for food");
        let mut stocked = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut stocked);
        let bank_com = (stocked.tables.com_id)("bank_main:inv").expect("bank com in dumps");
        stocked.world.inv.insert(bank_com, [(0u8, (315u16, 3u32))].into_iter().collect());
        assert!(bank_stocks_food(&stocked), "banked shrimps restock");
    }
}
