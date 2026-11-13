//! What a stuck item takes down with it (user decision 2026-11-13).
//!
//! When a node fails for good because an item cannot be obtained (smelting
//! the bronze bars of a bronze axe failed three times), every task that
//! depends on that item goes: woodcutting (the axe is its tool), and
//! firemaking when the bot meant to burn the logs it would chop — not just
//! the goal's own skill. Before any ban, each abandoned obtain on the way
//! (the failed root first, then what waited on it) is checked for a market
//! route (tradeable, not Iron, the wallet covers the ask or the reference
//! price): the first one found is bought instead (`lifecycle::retarget_market`)
//! and nothing is banned. Pure: the wallet and prices come from the brain.

use std::collections::BTreeSet;

use crate::model::{Life, Node, NodeKind, NodeStatus};

/// Skill a node trains or works (`None`: quests, coins, equipment).
pub fn node_skill(node: &Node) -> Option<String> {
    if let NodeKind::ReachLevel { skill, .. } = &node.kind {
        return Some(skill.clone());
    }
    let method = node.method.clone().or_else(|| node.alternatives.iter().map(|alt| alt.method.clone()).find(|method| !method.starts_with("market:") && !method.starts_with("shop:")));
    if let Some(skill) = method.as_deref().and_then(method_skill) {
        return Some(skill);
    }
    match &node.kind {
        NodeKind::ObtainItems { item, .. } => crate::obtain_legs::production_legs(item).into_iter().find_map(|leg| leg.skill.map(|(skill, _)| skill)),
        _ => None,
    }
}

/// Skill of a method, `farm:` legs included (fish, ore, logs, drops).
fn method_skill(method: &str) -> Option<String> {
    if let Some(item) = method.strip_prefix("farm:") {
        let skill = if crate::mapping::is_fish(item) {
            "fishing"
        } else if item.ends_with("_ore") || item == "coal" || item == "clay" || item == "blankrune" {
            "mining"
        } else if item.ends_with("logs") {
            "woodcutting"
        } else {
            "attack"
        };
        return Some(skill.to_string());
    }
    crate::ambition_catalog::method_skill(method).map(str::to_string)
}

/// Skills whose work needs `item` as gear (tool or bait family): the
/// bronze axe is woodcutting's, feathers are fly fishing's.
pub fn tool_skills(item: &str) -> Vec<String> {
    const GEARED: [&str; 10] = [
        "gather:woodcut/logs",
        "gather:mining/copper_ore",
        "gather:fishing/raw_shrimp",
        "gather:fishing/raw_sardine",
        "gather:fishing/raw_trout",
        "gather:fishing/raw_lobster",
        "gather:fishing/raw_tuna",
        "gather:sheep/wool",
        "fight:ranged/chickens",
        "fight:magic/chickens",
    ];
    let mut out: BTreeSet<String> = BTreeSet::new();
    for method in GEARED {
        let Some(need) = crate::tools::required_tool(method) else { continue };
        let names: Vec<&str> = crate::tools::tool_family(need.tool).iter().chain(need.bait.map(crate::tools::tool_family).unwrap_or(&[]).iter()).copied().collect();
        if names.contains(&item) {
            if let Some(skill) = method_skill(method) {
                out.insert(skill);
            }
        }
    }
    out.into_iter().collect()
}

/// `root` and every node that waits on it, transitively (positions,
/// root first, breadth first).
pub fn dependents(life: &Life, root: &str) -> Vec<usize> {
    let Some(start) = life.nodes.iter().position(|node| node.id == root) else { return Vec::new() };
    let mut order = vec![start];
    let mut cursor = 0;
    while cursor < order.len() {
        let id = life.nodes[order[cursor]].id.clone();
        for (position, node) in life.nodes.iter().enumerate() {
            if node.deps.contains(&id) && !order.contains(&position) {
                order.push(position);
            }
        }
        cursor += 1;
    }
    order
}

/// Every skill the failed `root` takes down: the skills of the root and
/// of each node waiting on it, plus the skills whose gear the root's
/// item is. Sorted, unique.
pub fn banned_scope(life: &Life, root: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for position in dependents(life, root) {
        let node = &life.nodes[position];
        if let Some(skill) = node_skill(node) {
            out.insert(skill);
        }
        if let NodeKind::ObtainItems { item, .. } = &node.kind {
            out.extend(tool_skills(item));
        }
    }
    out.into_iter().collect()
}

/// True when a node already fell back to the market (rescued once):
/// never rescued twice, the ban follows.
pub fn market_only(node: &Node) -> bool {
    !node.alternatives.is_empty() && node.alternatives.iter().all(|alt| alt.method.starts_with("market:"))
}

/// Abandoned obtains on the failed path, in rescue order (the root
/// first, then what waited on it): never the goal's own production
/// quota (a `produce` item is made, never bought), never one already
/// rescued. Empty for Iron (never trades).
pub fn rescue_candidates(life: &Life, root: &str) -> Vec<usize> {
    if life.profile.ironman {
        return Vec::new();
    }
    let quota = match &life.goal {
        crate::model::GoalKind::Produce { item, .. } => Some(item.clone()),
        _ => None,
    };
    dependents(life, root)
        .into_iter()
        .filter(|position| {
            let node = &life.nodes[*position];
            node.status == NodeStatus::Abandoned
                && !market_only(node)
                && matches!(&node.kind, NodeKind::ObtainItems { item, .. } if Some(item) != quota.as_ref() && crate::obtain_legs::tradeable_f2p(item))
        })
        .collect()
}

/// A market route exists: the wallet covers `need` units at the best
/// ask, else at the reference price (a resting bid then waits a seller).
pub fn market_route(need: i64, coins: i64, best_ask: Option<i64>, reference: i64) -> bool {
    let unit = best_ask.unwrap_or(reference);
    unit > 0 && unit.saturating_mul(need.max(1)) <= coins
}

#[cfg(test)]
#[path = "bans_scope_tests.rs"]
mod tests;
