//! Dependency cycles a block would close, broken by an alternative
//! (user 2026-11-17, live jim_ellis: a funded axe's `have_coins` hunted
//! men, blocked on herring; the herring obtain could only buy, blocked
//! on coins; a fresh `have_coins` hunted men again — one node pair every
//! 8 s, 300+ nodes, nothing earned).
//!
//! A node blocked on a need that a node WAITING on it already works
//! toward (the same object, coins, a skill or combat level, a quest, an
//! area) waits on itself, directly or through the coins: filing a fresh
//! twin repeats the pair forever. The cycle is broken at once, typed
//! (`FailureFold::CycleBroken`, the brain logs the line), at its OUTERMOST
//! node: among the twins and the earlier repeats of the blocked node's
//! own work (an unrolled chain: `have_coins` → herring → `have_coins` …),
//! the farthest one waiting on it whose blocking method is known
//! (`Node::blocked_by`), else the farthest. Its chain down to the blocked
//! node (included) leaves the graph — all of it served the method that
//! cannot work — and that method is excluded on it (`Node::excluded`):
//! its own blocking method, else, for a repeat of the blocked node, the
//! blocked node's method (the one a repeat resolves to). It resolves
//! another method. No method known: it resolves afresh (the next block
//! records one).
//!
//! Twins: a combat climb counts whatever its history (ryan_mason: a
//! climb blocked on runes whose obtain needs that combat level); any
//! other only once IT blocked — a climb whose next tranche's bar obtain
//! waits on it by plan is no cycle (owen_wood n11/n16).

use std::collections::HashSet;

use crate::lifecycle::FailureFold;
use crate::model::{Life, NodeKind, NodeStatus};

fn open(status: &NodeStatus) -> bool {
    !matches!(status, NodeStatus::Done | NodeStatus::Abandoned)
}

/// True when a node of `kind` works toward `need` (a typed block need:
/// `coins:N`, `combat:N`, `level:skill:N`, `quest:key`, `access:area`, or
/// an object).
pub fn works_toward(kind: &NodeKind, need: &str) -> bool {
    if let Some(raw) = need.strip_prefix("combat:") {
        return raw.parse::<u8>().is_ok_and(|level| matches!(kind, NodeKind::ReachCombat { level: aimed } if *aimed >= level));
    }
    if need.starts_with("coins:") {
        return matches!(kind, NodeKind::HaveCoins { .. } | NodeKind::MakeMoney);
    }
    if let Some(quest) = need.strip_prefix("quest:") {
        return matches!(kind, NodeKind::CompleteQuest { quest: aimed } if aimed == quest);
    }
    if let Some(area) = need.strip_prefix("access:") {
        return matches!(kind, NodeKind::AccessArea { area: aimed } if aimed == area);
    }
    if let Some(rest) = need.strip_prefix("level:") {
        let Some((skill, raw)) = rest.rsplit_once(':') else { return false };
        return raw.parse::<u8>().is_ok_and(|level| matches!(kind, NodeKind::ReachLevel { skill: aimed_skill, level: aimed } if aimed_skill == skill && *aimed >= level));
    }
    matches!(kind, NodeKind::ObtainItems { item, .. } if item == need)
}

/// Positions of the open nodes waiting on node `index` (transitively,
/// nearest first, the node itself left out) that already work toward
/// `need`.
pub fn twins(life: &Life, index: usize, need: &str) -> Vec<usize> {
    let me = life.nodes[index].id.clone();
    crate::bans_scope::dependents(life, &me)
        .into_iter()
        .filter(|&at| at != index)
        .filter(|&at| {
            let node = &life.nodes[at];
            open(&node.status) && works_toward(&node.kind, need) && (matches!(node.kind, NodeKind::ReachCombat { .. }) || node.blocks > 0)
        })
        .collect()
}

/// A coin node about to resolve while a BLOCKED coin node waits on it
/// (transitively): earning coins waits on earning coins, a cycle through
/// the coins (graphs unrolled before `record_block_from` broke them:
/// live jim_ellis, 680 nodes of `have_coins` → herring → `have_coins`).
/// The waiting coin nodes, nearest first; empty when none or not a coin
/// node. Break with `break_cycle(life, index, "coins", &found)`.
pub fn coin_regress(life: &Life, index: usize) -> Vec<usize> {
    if !matches!(life.nodes[index].kind, NodeKind::HaveCoins { .. } | NodeKind::MakeMoney) {
        return Vec::new();
    }
    twins(life, index, "coins:0")
}

/// Ids `root` waits on, transitively (the root left out).
fn below(life: &Life, root: &str) -> HashSet<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut frontier: Vec<String> = life.nodes.iter().filter(|node| node.id == root).flat_map(|node| node.deps.clone()).collect();
    while let Some(id) = frontier.pop() {
        if seen.insert(id.clone()) {
            frontier.extend(life.nodes.iter().filter(|node| node.id == id).flat_map(|node| node.deps.clone()));
        }
    }
    seen
}

/// True when two nodes do the same work (coins of any amount, the same
/// object, the same skill or quest): an unrolled chain repeats its node.
fn same_work(left: &NodeKind, right: &NodeKind) -> bool {
    match (left, right) {
        (NodeKind::HaveCoins { .. } | NodeKind::MakeMoney, NodeKind::HaveCoins { .. } | NodeKind::MakeMoney) => true,
        (NodeKind::ObtainItems { item: a, .. }, NodeKind::ObtainItems { item: b, .. }) => a == b,
        (NodeKind::ReachLevel { skill: a, .. }, NodeKind::ReachLevel { skill: b, .. }) => a == b,
        (NodeKind::ReachCombat { .. }, NodeKind::ReachCombat { .. }) => true,
        (NodeKind::CompleteQuest { quest: a }, NodeKind::CompleteQuest { quest: b }) => a == b,
        (NodeKind::AccessArea { area: a }, NodeKind::AccessArea { area: b }) => a == b,
        _ => false,
    }
}

/// Break the cycle node `index` would close on `need` (`twins`: the
/// nodes waiting on it that work toward it, nearest first). Returns the
/// typed line the brain logs. The blocked node leaves the graph unless
/// something outside the cycle waits on it: the caller never reads
/// `life.nodes[index]` afterwards.
pub fn break_cycle(life: &mut Life, index: usize, need: &str, twins: &[usize]) -> FailureFold {
    if twins.is_empty() {
        return FailureFold::Retry;
    }
    let me = life.nodes[index].id.clone();
    let nearest = life.nodes[twins[0]].id.clone();
    let own_method = life.nodes[index].method.clone().filter(|method| !method.is_empty());
    // The cycle's nodes waiting on `me`, nearest first: the twins and the
    // earlier repeats of `me` (blocked ones: a repeat by plan is no loop).
    let my_kind = life.nodes[index].kind.clone();
    let members: Vec<usize> = crate::bans_scope::dependents(life, &me)
        .into_iter()
        .filter(|&at| at != index)
        .filter(|&at| twins.contains(&at) || (open(&life.nodes[at].status) && life.nodes[at].blocks > 0 && same_work(&life.nodes[at].kind, &my_kind)))
        .collect();
    let pick = members.iter().rev().copied().find(|&at| life.nodes[at].blocked_by.is_some()).unwrap_or(members[members.len() - 1]);
    let repeat = same_work(&life.nodes[pick].kind, &my_kind);
    let twin = life.nodes[pick].id.clone();
    let under_twin = below(life, &twin);
    let chain: HashSet<String> = crate::bans_scope::dependents(life, &me)
        .into_iter()
        .map(|at| life.nodes[at].id.clone())
        .filter(|id| under_twin.contains(id))
        .collect();
    life.nodes[pick].deps.retain(|dep| !chain.contains(dep));
    // Chain members nobody outside the chain waits on leave, then the open
    // nodes only they waited on.
    let mut gone: HashSet<String> = chain.clone();
    loop {
        let kept: Vec<String> = gone
            .iter()
            .filter(|id| life.nodes.iter().any(|node| !gone.contains(&node.id) && node.deps.contains(id)) || life.active.as_deref() == Some(id.as_str()))
            .cloned()
            .collect();
        if kept.is_empty() {
            break;
        }
        for id in kept {
            gone.remove(&id);
        }
    }
    loop {
        let orphans: Vec<String> = life
            .nodes
            .iter()
            .filter(|node| !gone.contains(&node.id) && node.id != twin && open(&node.status) && node.status != NodeStatus::Active)
            .filter(|node| {
                let mut waiting = life.nodes.iter().filter(|other| other.deps.contains(&node.id)).peekable();
                waiting.peek().is_some() && waiting.all(|other| gone.contains(&other.id))
            })
            .map(|node| node.id.clone())
            .collect();
        if orphans.is_empty() {
            break;
        }
        gone.extend(orphans);
    }
    let excluded = life.nodes[pick].blocked_by.clone().or_else(|| own_method.clone().filter(|_| repeat));
    {
        let node = &mut life.nodes[pick];
        if let Some(method) = &excluded {
            if !node.excluded.contains(method) {
                node.excluded.push(method.clone());
            }
        }
        node.method = None;
        node.status = NodeStatus::Pending;
    }
    if !gone.contains(&me) {
        // Something else still waits on it: it stays, out of the cycle.
        let node = &mut life.nodes[index];
        node.method = None;
        node.status = NodeStatus::Pending;
    }
    let mut dropped: Vec<&String> = gone.iter().collect();
    dropped.sort_by_key(|id| crate::lifecycle_retire::node_number(id).unwrap_or(usize::MAX));
    let span = match (dropped.first(), dropped.last()) {
        (Some(first), Some(last)) if first != last => format!(" ({first}..{last})"),
        (Some(only), _) => format!(" ({only})"),
        _ => String::new(),
    };
    let method = excluded.as_deref().map(|method| format!(" ({method})")).unwrap_or_default();
    let verdict = match &excluded {
        Some(method) => format!("{method} excluded"),
        None => "blocking method unknown, resolved afresh".to_string(),
    };
    let line = format!("cycle broken: {me} needs {need}, which {nearest} works toward while waiting on {me}; broken at {twin}{method}: {verdict}, {} node(s) dropped{span}", dropped.len());
    life.nodes[pick].last_error = line.clone();
    if life.engagement.as_ref().is_some_and(|engaged| gone.contains(&engaged.node)) {
        life.engagement = None;
    }
    life.nodes.retain(|node| !gone.contains(&node.id));
    FailureFold::CycleBroken(line)
}

#[cfg(test)]
#[path = "lifecycle_cycle_tests.rs"]
mod tests;
