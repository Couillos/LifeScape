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
//!                     more, or is on an ambition whose open milestone is
//!                     exactly (s, g) and is within the A1 horizon of it
//!                     (`climb_ticks` at the best XP rate under
//!                     `INVESTMENT_HORIZON_TICKS`); else 0 (binary: a
//!                     crafter at 20 covers nothing, 8 low crafters do not
//!                     make crafting 70 "covered" by their XP share)
//! weight(kind, P)   = serves(kind, P) x asked x product of coverage of the
//!                     steps the kind does not train
//!                     (serves = 1 when a step the kind trains is uncovered)
//! ```
//!
//! Protected accounts, operator ambitions and market makers cover nothing
//! (they serve no demand), the bot being valued is left out (coverage is
//! by the OTHERS). Pure: the roster and the open level milestones
//! (`targets`: account, skill, level) come in as arguments.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::ambition_fleet::FleetBot;
use crate::investment::{climb_ticks, INVESTMENT_HORIZON_TICKS};

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

/// Whether the step `(skill, level)` is covered by the roster's bots
/// other than `exclude`: 1.0 or 0.0 (module doc).
pub fn coverage(roster: &[FleetBot], targets: &[(String, String, u8)], exclude: &str, skill: &str, level: u8, xp_rate: &dyn Fn(&str, u8) -> Option<f64>) -> f64 {
    let covered = roster.iter().filter(|other| !other.account.eq_ignore_ascii_case(exclude) && !crate::ambition_value::protected(&other.account) && !other.operator && !other.market_maker).any(|other| {
        let have = other.level(skill);
        if have >= level {
            return true;
        }
        let aims = targets.iter().any(|(account, target_skill, target_level)| account.eq_ignore_ascii_case(&other.account) && target_skill == skill && *target_level == level);
        aims && climb_ticks(skill, have, level, xp_rate).is_some_and(|ticks| ticks < INVESTMENT_HORIZON_TICKS)
    });
    if covered {
        1.0
    } else {
        0.0
    }
}
