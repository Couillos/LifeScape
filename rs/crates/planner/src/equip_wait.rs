//! Equipment goals that wait on gear nobody makes (user 2026-11-21, plan
//! progression A3). Live: the mages' `equip` milestone filed an obtain of
//! the amulet of power, nobody produced it, the goal closed stuck three
//! times and the ambition fell. A piece that is neither held, nor
//! obtainable now (a resting ask, an open shop, a way some fleet bot has
//! the level for), nor made by anyone is no failure of the bot: the
//! milestone waits, cooled, with no block counted and no skill banned,
//! and the bot works its other milestones. Pure.

use crate::model::{GoalKind, Life, NodeKind, NodeStatus};

/// The first piece of a stuck `equip` goal that waits on gear nobody
/// makes: an obtain the goal's `equip_best` depends on, not done, whose
/// item is not `held` (pack, bank, worn) and not `obtainable` now. `None`
/// for any other goal, or when every missing piece has a way.
pub fn unmade_equip_piece(life: &Life, held: &dyn Fn(&str) -> bool, obtainable: &dyn Fn(&str) -> bool) -> Option<String> {
    let GoalKind::EquipBest { style: goal_style } = &life.goal else { return None };
    life.nodes
        .iter()
        .filter(|node| matches!(&node.kind, NodeKind::EquipBest { style } if style == goal_style) && !crate::gear_ladder::is_ladder_equip(node))
        .flat_map(|equip| equip.deps.iter())
        .filter_map(|dep| life.nodes.iter().find(|node| &node.id == dep))
        .filter(|node| node.status != NodeStatus::Done)
        .filter_map(|node| match &node.kind {
            NodeKind::ObtainItems { item, .. } => Some(item.as_str()),
            _ => None,
        })
        .find(|item| !held(item) && !obtainable(item))
        .map(str::to_string)
}

#[cfg(test)]
#[path = "equip_wait_tests.rs"]
mod tests;
