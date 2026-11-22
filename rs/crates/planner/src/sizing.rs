//! Tranche sizing (audit 2026-11-13 item A9, plan rendements R3.3): how
//! many actions a reach tranche takes, how long, and what those actions
//! burn — recipe inputs, bait, feathers, arrows, runes — from the XP the
//! tranche must earn and the method's YIELD (`work_rate`): XP per action,
//! ticks per XP and units burned per XP all read from one rate, never
//! from a constant of this file.
//!
//! Server truth (rev 274), through the rate:
//! - XP table in tenths (`gamedata::xp`), unit XP from the dumps
//!   (`unit_xp`): a product, an input eaten, a cast, a damage point;
//! - fights: the server's hit rolls (`method_rate_fight`: ranged and
//!   magic attack against the NPC's ranged or magic defence roll), every
//!   shot an arrow (the scripts never pick them back up), every cast the
//!   spell's runes;
//! - failed actions (iron smelts, burnt fish) burn their inputs without
//!   XP: the rate's flows count every attempt, its XP the successes.
//!
//! The planner reads the game prior (`work_rate::prior_rate`); a caller
//! holding measured rates passes its reader (`*_with`). A climb is
//! summed level by level (the odds rise with the level).

use crate::work_rate::{prior_rate, WorkRate};

/// Display XP between two levels (server table in tenths).
pub fn xp_between(from: u8, to: u8) -> f64 {
    let at = |level: u8| lifescape_gamedata::xp::xp_for_level(u32::from(level.clamp(1, 99))).unwrap_or(0);
    ((at(to) - at(from)).max(0)) as f64 / 10.0
}

/// The skill a method's level climb trains (ranged and magic fights by
/// their style, spells magic, recipes their way's gate). `None` for a
/// melee fight (its skill rides in the node) and a way without XP.
pub fn trained_skill(method: &str) -> Option<String> {
    if method.starts_with("fight:ranged/") {
        return Some("ranged".to_string());
    }
    if method.starts_with("fight:magic/") || method.starts_with("alch:") {
        return Some("magic".to_string());
    }
    if method.starts_with("fight:") {
        return None;
    }
    crate::method_way::method_way(method).or_else(|| crate::spell_methods::spell_way(method)).and_then(|(_, way)| way.skill.map(|(skill, _)| skill))
}

/// XP one action of `method` pays at `level` read from `rate` (an
/// attempt as the yield counts it; failures averaged in).
pub fn xp_per_action_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, level: u8) -> Option<f64> {
    let skill = trained_skill(method)?;
    rate(method, level.max(1))?.xp_per_action(&skill)
}

/// XP one action of `method` pays at `level` (the game prior).
pub fn xp_per_action(method: &str, level: u8) -> Option<f64> {
    xp_per_action_with(&prior_rate, method, level)
}

/// Actions a tranche from `from` to `to` takes with `method` (`None`: no
/// rate or no XP for the method, e.g. melee fights, which burn nothing).
pub fn tranche_actions(method: &str, from: u8, to: u8) -> Option<i64> {
    tranche_actions_with(&prior_rate, method, from, to)
}

/// `tranche_actions` read from `rate`.
pub fn tranche_actions_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, from: u8, to: u8) -> Option<i64> {
    // The method must pay XP at its start, even for an empty climb.
    xp_per_action_with(rate, method, from.max(1)).filter(|per| *per > 0.0)?;
    let mut actions = 0.0;
    for level in from.max(1)..to.min(99) {
        let per = xp_per_action_with(rate, method, level).filter(|per| *per > 0.0)?;
        actions += xp_between(level, level + 1) / per;
    }
    Some((actions - 1e-6).ceil().max(0.0) as i64)
}

/// Ticks of work a tranche from `from` to `to` takes with `method`, level
/// by level at `rate`'s ticks per XP (`None`: a level without a rate).
pub fn tranche_ticks_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, from: u8, to: u8) -> Option<f64> {
    let skill = trained_skill(method)?;
    let mut ticks = 0.0;
    for level in from.max(1)..to.min(99) {
        ticks += xp_between(level, level + 1) * rate(method, level)?.ticks_per_xp(&skill)?;
    }
    Some(ticks)
}

/// `tranche_ticks_with` on the game prior.
pub fn tranche_ticks(method: &str, from: u8, to: u8) -> Option<f64> {
    tranche_ticks_with(&prior_rate, method, from, to)
}

/// What a tranche burns at `rate`, (object, units): each consumable the
/// rate spends per XP (inputs over the success odds, a fight's arrows,
/// the spell's runes, bait, thread), level by level. Tools are not
/// consumption (`tools::required_tool`).
pub fn tranche_consumption_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, from: u8, to: u8) -> Vec<(String, i64)> {
    let Some(skill) = trained_skill(method) else { return Vec::new() };
    let mut sums: Vec<(String, f64)> = Vec::new();
    let mut actions = 0.0;
    for level in from.max(1)..to.min(99) {
        let Some(read) = rate(method, level) else { return Vec::new() };
        let climbed = xp_between(level, level + 1);
        let Some(per_action) = read.xp_per_action(&skill) else { return Vec::new() };
        actions += climbed / per_action;
        for (item, per_xp) in read.burned_per_xp(&skill) {
            match sums.iter_mut().find(|(known, _)| *known == item) {
                Some(known) => known.1 += per_xp * climbed,
                None => sums.push((item, per_xp * climbed)),
            }
        }
    }
    // Whole actions: the climb's last action burns its full share (three
    // air runes a teleport, never two), a float a hair above an integer
    // is that integer.
    let whole = (actions - 1e-6).ceil().max(0.0);
    let scale = if actions > 0.0 { whole / actions } else { 0.0 };
    sums.into_iter().map(|(item, units)| (item, (units * scale - 1e-6).ceil().max(0.0) as i64)).filter(|(_, units)| *units > 0).collect()
}

/// `tranche_consumption_with` on the game prior.
pub fn tranche_consumption(method: &str, from: u8, to: u8) -> Vec<(String, i64)> {
    tranche_consumption_with(&prior_rate, method, from, to)
}

/// Units of `need` one action of `method` at `level` burns (an attempt:
/// one object an alchemy, the spell's runes a cast, the way's inputs a
/// recipe), rounded up.
pub fn per_action_need(method: &str, level: u8, need: &str) -> Option<i64> {
    per_action_need_with(&prior_rate, method, level, need)
}

/// `per_action_need` read from `rate`.
pub fn per_action_need_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, level: u8, need: &str) -> Option<i64> {
    let skill = trained_skill(method)?;
    let read = rate(method, level.max(1))?;
    let per_action = read.xp_per_action(&skill)?;
    read.burned_per_xp(&skill).into_iter().find(|(item, _)| item == need).map(|(_, per_xp)| (per_xp * per_action - 1e-6).ceil().max(1.0) as i64)
}

/// Units of `need` a tranche of `method` from `from` to `to` burns
/// (`None` when the tranche does not consume it).
pub fn tranche_need(method: &str, from: u8, to: u8, need: &str) -> Option<i64> {
    tranche_need_with(&prior_rate, method, from, to, need)
}

/// `tranche_need` read from `rate`.
pub fn tranche_need_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, from: u8, to: u8, need: &str) -> Option<i64> {
    tranche_consumption_with(rate, method, from, to).into_iter().find(|(item, _)| item == need).map(|(_, units)| units)
}

/// Units of `need` ONE work session of `method` burns at `level`: the
/// consumption of one 6000-tick window (`WINDOW_TICKS`, the normal work
/// budget) read from `rate`, a random recipe sized with its margin
/// (`input_margin`). A climb buys this much at a time, never its whole
/// tranche (live 2026-11-22: 12 447 iron bars bid against a market that
/// makes 450 an hour). `None`: the method does not consume `need`.
pub fn session_need_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, level: u8, need: &str) -> Option<i64> {
    let skill = trained_skill(method)?;
    let read = rate(method, level.max(1))?;
    let earned = read.xp_of(&skill);
    let per_xp = read.burned_per_xp(&skill).into_iter().find(|(item, _)| item == need).map(|(_, per_xp)| per_xp)?;
    let units = per_xp * earned;
    if units <= 0.0 {
        return None;
    }
    let per_action = per_action_need_with(rate, method, level, need).unwrap_or(1);
    Some(crate::input_margin::sized_inputs(units, per_action, crate::input_margin::recipe_success(method)).max(1))
}

/// Actions one work session of `method` makes at `level` (`None`: no rate).
pub fn session_actions_with(rate: &dyn Fn(&str, u8) -> Option<WorkRate>, method: &str, level: u8) -> Option<i64> {
    let read = rate(method, level.max(1))?;
    (read.actions_6000 > 0.0).then(|| read.actions_6000.ceil().max(1.0) as i64)
}

#[cfg(test)]
#[path = "sizing_tests.rs"]
mod tests;
