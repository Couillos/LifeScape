//! Chain coverage for the A4 value (2026-11-21). A demand whose chain
//! crosses a step nobody covers (the strung diamond amulet needs crafting
//! 70, and nobody has it) is not served by the kind that makes only its
//! LAST step: live, the fleet's mages were given the enchant of the amulet
//! of power while no crafter made the diamond amulets, 0 amulets in all.
//! The value of the demand goes to the uncovered step instead.
//!
//! ```text
//! steps(item)       = (skill, level) of the item's easiest skilled way and,
//!                     recursively, of the inputs of that way
//! coverage(s, g)    = 1 when another working bot has the skill s at g or
//!                     more; else the best progress of a bot on a kind that
//!                     trains s: xp(level) / xp(g), the share of the XP the
//!                     step asks that it already holds (0 when nobody is on
//!                     it; the XP is back-loaded: crafting 66 holds ~70 % of
//!                     the XP of 70, crafting 20 almost none)
//! weight(kind, P)   = serves(kind, P) × asked × Π coverage of the steps
//!                     the kind does not train
//!                     (serves = 1 when a step the kind trains is uncovered)
//! ```
//!
//! Protected accounts, operator ambitions and market makers cover nothing
//! (they serve no demand), the bot being valued is left out (coverage is
//! by the OTHERS). Bots already on a step and their progress count.
//! Pure: the roster comes in as an argument.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::ambition_fleet::FleetBot;

/// Deepest chain of inputs followed.
const STEP_DEPTH: u8 = 4;

/// Skilled steps of making `item`: (skill, level), the highest level per
/// skill, along the easiest skilled way and the ways of its inputs.
/// Static game data, memoised.
pub fn chain_steps(item: &str) -> Vec<(String, u8)> {
    static MEMO: OnceLock<Mutex<HashMap<String, Vec<(String, u8)>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(known) = memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(item) {
        return known.clone();
    }
    let mut out = Vec::new();
    collect(item, STEP_DEPTH, &mut out);
    memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(item.to_string(), out.clone());
    out
}

fn collect(item: &str, depth: u8, out: &mut Vec<(String, u8)>) {
    let legs = crate::obtain_legs::production_legs(item);
    let easiest = legs.iter().filter(|leg| leg.skill.is_some() && leg.quest.is_none() && !leg.method.starts_with("ground:")).min_by_key(|leg| leg.skill.as_ref().map(|(_, level)| *level).unwrap_or(0));
    let Some(leg) = easiest else { return };
    if let Some((skill, level)) = &leg.skill {
        let level = u8::try_from((*level).clamp(1, 99)).unwrap_or(99);
        match out.iter_mut().find(|(known, _)| known == skill) {
            Some(known) => known.1 = known.1.max(level),
            None => out.push((skill.clone(), level)),
        }
    }
    if depth == 0 {
        return;
    }
    for (input, _) in &leg.inputs {
        collect(input, depth - 1, out);
    }
}

/// How much of the step `(skill, level)` the roster's bots other than
/// `exclude` cover, in [0, 1] (module doc).
pub fn coverage(roster: &[FleetBot], exclude: &str, skill: &str, level: u8) -> f64 {
    let mut best: f64 = 0.0;
    for other in roster.iter().filter(|other| !other.account.eq_ignore_ascii_case(exclude) && !crate::ambition_value::protected(&other.account) && !other.operator && !other.market_maker) {
        let have = other.level(skill);
        if have >= level {
            return 1.0;
        }
        let on_step = other.kind.as_deref().is_some_and(|kind| crate::ambition_value::own_skills(kind).contains(&skill));
        if !on_step {
            continue;
        }
        let asked = crate::sizing::xp_between(1, level);
        if asked > 0.0 {
            best = best.max((crate::sizing::xp_between(1, have.max(1)) / asked).min(1.0));
        }
    }
    best
}
