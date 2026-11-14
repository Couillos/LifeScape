//! Quest objects on hold (user rule 2026-11-14: "il faut les items
//! prérequis avant de lancer une quête", and never a silent loop).
//!
//! Live 2026-11-14 (bh99tw2f, a merchant): Rune Mysteries started, the
//! Duke's air talisman went to the bank, and the bot's own listings
//! locked all ten banked talismans. The Duke never re-issues one while
//! any is held (`~obj_gettotal`), Sedridor answered "...except I don't
//! have it with me" four rounds a launch, three launches a goal, the
//! failure banned mining, and the next goal planned the quest again.
//!
//! Three rules, pure (counts and the clock come from the brain):
//! - while a quest node is open, the objects it needs are reserved
//!   (`reserved_quest_objects`): the sales pass never lists them and
//!   releases the listings that lock them, the market refuses new ones;
//! - a quest launches only with its objects at hand (`quest_shortfalls`):
//!   missing brought items are obtained first (graph nodes), units held
//!   but locked by a sale are released first; a handed object held
//!   nowhere is fine (the quest's NPC hands it at its stage);
//! - a quest that fails for good cools down as a quest
//!   (`quest_ban_key`), never as the skills of the goal waiting on it.

use crate::model::{HistoryEntry, Life, NodeKind, NodeStatus};

/// Ban key prefix of a cooled quest in the goal history
/// (`stuck:quest:<key>@<ticks>:...`, read back by `bans::ban_states`).
pub const QUEST_BAN_PREFIX: &str = "quest:";

/// Quest keys of the open (neither done nor abandoned) quest nodes, the
/// tutorial excluded, unique, in graph order.
pub fn open_quests(life: &Life) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in &life.nodes {
        if matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned) {
            continue;
        }
        if let NodeKind::CompleteQuest { quest } = &node.kind {
            if quest != "tutorial" && !out.contains(quest) {
                out.push(quest.clone());
            }
        }
    }
    out
}

/// Objects the open quests keep on hand, `(debug name, units)` summed
/// across quests, sorted by name (`gamedata::quest_requirements::
/// reserved_objects`: brought items at their count, one unit of each
/// handed object).
pub fn reserved_quest_objects(life: &Life) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    for quest in open_quests(life) {
        for (name, units) in lifescape_gamedata::quest_requirements::reserved_objects(&quest) {
            match out.iter_mut().find(|(held, _)| *held == name) {
                Some(slot) => slot.1 = slot.1.saturating_add(units),
                None => out.push((name, units)),
            }
        }
    }
    out.sort();
    out
}

/// Stacks `(name, units)` net of the reservation: what a sale may offer.
/// Stacks that fall to zero leave the list.
pub fn withhold_reserved(stacks: &[(String, i64)], reserved: &[(String, i64)]) -> Vec<(String, i64)> {
    stacks
        .iter()
        .map(|(name, units)| {
            let kept = reserved.iter().find(|(held, _)| held == name).map(|(_, units)| *units).unwrap_or(0);
            (name.clone(), units.saturating_sub(kept.max(0)))
        })
        .filter(|(_, units)| *units > 0)
        .collect()
}

/// Why a quest cannot launch yet.
#[derive(Debug, Clone, PartialEq)]
pub enum QuestShortfall {
    /// Fewer units held than the quest brings: obtained first.
    Missing { item: String, need: i64, held: i64 },
    /// Units held but promised to the bot's own market sales: those
    /// listings are released first.
    Locked { item: String, need: i64, held: i64, free: i64 },
}

impl QuestShortfall {
    /// Log and history wording: the object, the need and where it is.
    pub fn describe(&self) -> String {
        match self {
            QuestShortfall::Missing { item, need, held } => format!("{item}: {held} held, {need} needed"),
            QuestShortfall::Locked { item, need, held, free } => {
                format!("{item}: {held} held but {} locked by open market sales ({free} free, {need} needed)", held - free)
            }
        }
    }
}

/// Shortfalls of `quest` given `count_of(name) = (held, free)` (held:
/// pack + bank + worn; free: held minus what market sales lock). Brought
/// items need their count free; a handed object held anywhere needs one
/// unit free (its NPC re-issues only when none is held), held nowhere it
/// is the quest's to hand. Coins stay the wallet's. Empty: launch.
pub fn quest_shortfalls(quest: &str, count_of: &dyn Fn(&str) -> (i64, i64)) -> Vec<QuestShortfall> {
    let mut out = Vec::new();
    if let Some(required) = lifescape_gamedata::quest_requirements::by_key(quest) {
        for item in required.items.iter().filter(|item| item.item != "coins") {
            let (held, free) = count_of(&item.item);
            if free >= item.count {
                continue;
            }
            out.push(if held >= item.count {
                QuestShortfall::Locked { item: item.item.clone(), need: item.count, held, free }
            } else {
                QuestShortfall::Missing { item: item.item.clone(), need: item.count, held }
            });
        }
    }
    for name in lifescape_gamedata::quest_requirements::quest_objects(quest) {
        let (held, free) = count_of(name);
        if held > 0 && free < 1 {
            out.push(QuestShortfall::Locked { item: name.to_string(), need: 1, held, free });
        }
    }
    out
}

/// Ban key of a cooled quest.
pub fn quest_ban_key(quest: &str) -> String {
    format!("{QUEST_BAN_PREFIX}{quest}")
}

/// Quests cooling now (an active `quest:<key>` ban), sorted.
pub fn cooled_quests(history: &[HistoryEntry], ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    crate::bans::banned_skills(history, ticks_since)
        .into_iter()
        .filter_map(|key| key.strip_prefix(QUEST_BAN_PREFIX).map(str::to_string))
        .collect()
}

/// Ids of the ready quest nodes whose quest is cooling: the launch pass
/// skips them like parked market waits.
pub fn cooled_quest_nodes(life: &Life, cooled: &[String]) -> Vec<String> {
    life.nodes
        .iter()
        .filter(|node| node.status == NodeStatus::Ready)
        .filter(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if cooled.contains(quest)))
        .map(|node| node.id.clone())
        .collect()
}

/// The quest of node `id`, when it is a quest node.
pub fn node_quest(life: &Life, id: &str) -> Option<String> {
    life.nodes.iter().find(|node| node.id == id).and_then(|node| match &node.kind {
        NodeKind::CompleteQuest { quest } => Some(quest.clone()),
        _ => None,
    })
}

#[cfg(test)]
#[path = "quest_hold_tests.rs"]
mod tests;
