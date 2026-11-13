//! Quest prerequisites of planner methods (user decision 2026-11-13:
//! "the brain computes every dependency, quests included"). A method
//! whose quest the bot has not finished never launches: the resolver
//! takes another way, else blocks `missing:quest:<key>` and
//! `record_block_from` plans the quest as a dependency (its own skill and
//! item requirements expanded, `expand_quest_from`).
//!
//! Server truth (rev 274), read from the obtain index (`Way::quest`):
//! - rune essence: `skill_runecraft/scripts/essence_mine.rs2` refuses
//!   Aubury's teleport while `%runemysteries < ^runemysteries_complete`
//!   (`essence:`, `farm:blankrune`, `gather:mining/blankrune`);
//! - quest-gated shops (`f2p_shop_access` gates): Oziach's rune
//!   platebody needs Dragon Slayer — a `shop:buy` leg whose every shop
//!   is gated needs that quest;
//! - wielding (rune platebody: `levelrequire_dragon_slayer_quest_*`) is
//!   gated in `gamedata::gear` (`quests_done` filter);
//! - the Al Kharid toll is free after Prince Ali Rescue (`combat::Fare`).
//!
//! Completion: every completion varp of the quest at or past its final
//! value in the engine save (quest varps are never transmitted to the
//! client), or the life's own quest node done.

use crate::expand::ReadyCtx;
use crate::model::{Life, NodeKind, NodeStatus};

/// Planner quest key of an obtain-index quest directory (`quest_dragon`
/// → `dragon`).
fn quest_key(directory: &str) -> String {
    directory.strip_prefix("quest_").unwrap_or(directory).to_string()
}

/// Quest a method needs before it can run (`None`: none). `item` names
/// the obtained object for purchase legs (`shop:buy`).
pub fn method_quest(method: &str, item: Option<&str>) -> Option<String> {
    if method.starts_with("essence:") {
        return lifescape_gamedata::obtain::gather_quest("blankrune").map(quest_key);
    }
    let gathered = method
        .strip_prefix("farm:")
        .or_else(|| method.strip_prefix("gather:mining/"))
        .or_else(|| method.strip_prefix("gather:fishing/"))
        .or_else(|| method.strip_prefix("gather:woodcut/"));
    if let Some(item) = gathered {
        return lifescape_gamedata::obtain::gather_quest(item).map(quest_key);
    }
    if method == "shop:buy" {
        let item = item?;
        let shops: Vec<lifescape_gamedata::obtain::Way> =
            lifescape_gamedata::obtain::ways(item).into_iter().filter(|way| way.kind == lifescape_gamedata::obtain::WayKind::Shop).collect();
        // Gated only when EVERY shop selling it is gated.
        if !shops.is_empty() && shops.iter().all(|way| way.quest.is_some()) {
            return shops.iter().find_map(|way| way.quest.as_deref()).map(quest_key);
        }
    }
    None
}

/// The quest `method` needs and the bot has not finished, if any.
pub fn missing_quest(method: &str, item: Option<&str>, ctx: &ReadyCtx) -> Option<String> {
    method_quest(method, item).filter(|quest| !ctx.quests_done.iter().any(|done| done == quest))
}

/// Static effort of a quest for the quest-point ladder: a base, every
/// object to bring (its stack counts by tens), every skill gate by half its
/// level, half the recommended combat.
fn effort(quest: &lifescape_gamedata::quest_requirements::QuestRequirements) -> i64 {
    let items: i64 = quest.items.iter().map(|item| 1 + item.count.min(100) / 10).sum();
    let skills: i64 = quest.skills.iter().map(|gate| gate.level / 2).sum();
    10 + items + skills + quest.recommended_combat / 2
}

/// Quests that bring a bot from the points it holds (`done`) to `points`
/// quest points, `target` excluded: the cheapest points first (reward per
/// effort), a quest gated on points only once the rungs before it reach
/// its gate (Black Knights' Fortress after 12), never a quest no bot runs
/// solo (`blocker`: Shield of Arrav, Pirate's Treasure). Rungs come in
/// order; empty when `done` already holds the points. Replaces the old
/// collapse onto Cook's Assistant (1 point for a 32-point gate).
pub fn quest_point_ladder(points: i64, target: &str, done: &[String]) -> Vec<String> {
    let all = lifescape_gamedata::quest_requirements::all_f2p_quests();
    let mut held: i64 = all.iter().filter(|quest| done.contains(&quest.key)).map(|quest| quest.reward_points).sum();
    let mut picked: Vec<String> = Vec::new();
    while held < points {
        let next = all
            .iter()
            .filter(|quest| quest.key != target && !done.contains(&quest.key) && !picked.contains(&quest.key))
            .filter(|quest| quest.blocker.is_none() && quest.quest_points <= held && quest.reward_points > 0)
            .max_by(|left, right| {
                let score = |quest: &&lifescape_gamedata::quest_requirements::QuestRequirements| quest.reward_points as f64 / effort(quest) as f64;
                score(left).total_cmp(&score(right)).then_with(|| right.key.cmp(&left.key))
            });
        let Some(quest) = next else { break };
        held += quest.reward_points;
        picked.push(quest.key.clone());
    }
    picked
}

/// F2P quests complete in a save's varps (`varp_of(id)`, absent = 0):
/// every completion varp at or past its final value.
pub fn quests_done_from(varp_of: &dyn Fn(i64) -> i64) -> Vec<String> {
    lifescape_gamedata::quest_requirements::all_f2p_quests()
        .into_iter()
        .filter(|quest| !quest.completion_varps.is_empty() && quest.completion_varps.iter().all(|(varp, value)| varp_of(*varp) >= *value))
        .map(|quest| quest.key)
        .collect()
}

/// Quests the life's own graph finished (a done `complete_quest` node):
/// proof between two engine saves (autosave every 15 min).
pub fn quests_done_in_graph(life: &Life) -> Vec<String> {
    life.nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Done)
        .filter_map(|node| match &node.kind {
            NodeKind::CompleteQuest { quest } => Some(quest.clone()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "quests_tests.rs"]
mod tests;
