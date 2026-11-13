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
