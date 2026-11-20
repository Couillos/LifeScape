//! What a bot's ambition climbs NOW, for the bid response (2026-11-20,
//! « le smithing avance si lentement », « le marché doit s'équilibrer »).
//!
//! Before: the training a lot needs cost nothing when the ambition's
//! LADDER climbed the skill that high some day (`ladder_level`: rungs
//! 10 to 90, so every smithing level up to 90 for a smith). Live
//! 05:09 UTC: alice_knight, smithing 73, answered 28 adamant platebodies
//! (smithing 88) as a « 517-tick lot, 132 coins per tick »; the climb
//! 73 -> 88 (2.4 M XP) was filed, its market buys sized on it (29 004
//! coal, 5 717 adamantite ore as `need` bids) went unfilled, the goal
//! stuck after three market legs, the bids closed `need_gone`; the next
//! answer was the same. 13 bots waited on `level:smithing:66-88`.
//!
//! Rule: the levels a bot's ambition climbs now are its OPEN level
//! milestones (the rung it works), never the ladder's top; a level above
//! them and above the bot's own is a training the lot carries
//! (`market_value::training_share`). The answers drawn under the old
//! rule whose object needs such a level are given back
//! (`beyond_reach_answer`, the brain: `brain_bid_release`), the draw
//! that follows weighs them under the new rule. An answer drawn under the
//! new rule carries `PLANNED_RULE_TAG` (history, after its `next:`): the
//! cost of its training already decided (user 2026-11-19, « tout le
//! temps » : any gap is priced), it is never given back.
//! Pure: game data and the life only.

use crate::ambition::{MilestoneStatus, Target};
use crate::model::{GoalKind, Life, NodeKind, NodeStatus};

/// History outcome (node `goal`) the brain writes right after the draw of
/// a `bid:` answer weighed with `planned_levels` (module doc).
pub const PLANNED_RULE_TAG: &str = "bid_rule:planned_milestones";

/// True when the current goal was drawn under this rule (a
/// `PLANNED_RULE_TAG` entry after the latest goal draw).
pub fn drawn_under_rule(history: &[crate::model::HistoryEntry]) -> bool {
    for entry in history.iter().rev().filter(|entry| entry.node == "goal") {
        if entry.outcome == PLANNED_RULE_TAG {
            return true;
        }
        if entry.outcome.starts_with(crate::ambition_review::NEXT_TAG) {
            return false;
        }
    }
    false
}

/// The highest level each skill's open level milestones climb to (empty
/// without an ambition).
pub fn planned_levels(life: &Life) -> Vec<(String, u8)> {
    let Some(ambition) = life.ambition.as_ref() else { return Vec::new() };
    let mut out: Vec<(String, u8)> = Vec::new();
    for stone in ambition.milestones.iter().filter(|stone| stone.status != MilestoneStatus::Done) {
        if let Target::Level { skill, level } = &stone.target {
            match out.iter_mut().find(|(known, _)| known == skill) {
                Some(known) => known.1 = known.1.max(*level),
                None => out.push((skill.clone(), *level)),
            }
        }
    }
    out
}

/// A `bid:` answer to give back: its origin and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeyondReach {
    pub origin: String,
    pub why: String,
}

/// The current open `bid:` goal drawn under the old rule (no
/// `PLANNED_RULE_TAG`) when (a) every skilled way to make its object needs
/// a level above the bot's own (`level_of`) and above what its ambition
/// climbs now (`planned_levels`: the old rule counted that training
/// free), or (b) its graph still climbs a level (an open `reach_level`
/// node: its line was chosen and its inputs sized under the old training
/// score, `train_score` 2026-11-20, e.g. 40 060 copper ore for bronze bars
/// smelted to smithing 66). `None` otherwise: no such goal, the goal
/// closed, an answer drawn under this rule, a way within reach and
/// nothing left to climb.
pub fn beyond_reach_answer(life: &Life, level_of: &dyn Fn(&str) -> u8) -> Option<BeyondReach> {
    let origin = life.goal_origin.as_deref()?;
    let item = origin.strip_prefix(crate::bid_response::BID_PREFIX)?;
    if crate::goals::goal_closed(&life.history) || drawn_under_rule(&life.history) {
        return None;
    }
    let GoalKind::Produce { item: goal_item, .. } = &life.goal else { return None };
    if goal_item != item {
        return None;
    }
    if let Some(why) = beyond_levels(life, item, level_of) {
        return Some(BeyondReach { origin: origin.to_string(), why });
    }
    life.nodes.iter().find_map(|node| match (&node.kind, &node.status) {
        (_, NodeStatus::Done | NodeStatus::Abandoned | NodeStatus::Failed { .. }) => None,
        (NodeKind::ReachLevel { skill, level }, _) => Some(BeyondReach { origin: origin.to_string(), why: format!("{} still climbs {skill} to {level} on a line chosen under the old training score", node.id) }),
        _ => None,
    })
}

/// Why `item` is beyond the bot's level and its open milestones (case
/// (a) of `beyond_reach_answer`), the nearest way named; `None` when a
/// way is within reach or needs no skill.
fn beyond_levels(life: &Life, item: &str, level_of: &dyn Fn(&str) -> u8) -> Option<String> {
    let planned = planned_levels(life);
    let planned_of = |skill: &str| planned.iter().find(|(known, _)| known == skill).map(|(_, level)| *level).unwrap_or(0);
    let mut nearest: Option<(u8, String)> = None;
    for leg in &crate::obtain_legs::production_legs(item) {
        let (skill, needed) = leg.skill.as_ref()?;
        let needed = u8::try_from((*needed).clamp(1, 99)).unwrap_or(99);
        let reach = level_of(skill).max(planned_of(skill));
        if reach >= needed {
            return None;
        }
        if nearest.as_ref().is_none_or(|(gap, _)| needed - reach < *gap) {
            nearest = Some((needed - reach, format!("{skill} {needed} needed, the bot at {} and its milestones at {}", level_of(skill), planned_of(skill))));
        }
    }
    nearest.map(|(_, why)| why)
}

#[cfg(test)]
#[path = "bid_reach_tests.rs"]
mod tests;
