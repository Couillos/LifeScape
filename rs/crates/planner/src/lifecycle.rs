//! Node lifecycle: the single place that moves graph statuses. Every
//! mutation of a life (report fold, launch pass, resolve) ends with
//! `normalize`, which restores the invariants below; the brain never
//! patches statuses by hand. Pure: no clock, no files (history stamps
//! stay in the brain).
//!
//! Invariants after `normalize`:
//! - a node whose dependency is abandoned (or names no node, or sits in a
//!   dependency cycle) is abandoned, transitively;
//! - `Ready` ⇔ every dependency is `Done` (`Pending`/`Blocked` otherwise);
//! - at most one `Active` node, and it is `life.active`;
//! - liveness: a graph with an unfinished node has either an active node
//!   or a ready one — never an open graph with nothing to launch.

use std::collections::HashSet;

use crate::model::{Life, Node, NodeKind, NodeStatus};
use crate::replan::{replan_node, ReplanCause};

/// Runtime failures before a node is abandoned (typed, with dossier).
pub const MAX_FAILURES: u32 = 3;
/// Blocked stops (missing tool/input) before a node is abandoned: a node
/// that keeps blocking after its sub-tasks landed never heals alone.
pub const MAX_BLOCKS: u32 = 3;

/// What the launch pass may do with a normalized graph.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphState {
    /// A node is running (or awaiting its fill): nothing to launch.
    Running(String),
    /// At least one node is ready.
    Launchable,
    /// Every node is done (an empty graph counts as done).
    Finished,
    /// Nothing left to run and some nodes were abandoned: the operator
    /// decides (rule 7), the brain never revives on its own.
    Stuck { abandoned: Vec<String> },
}

/// Result of folding a failure into a node.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureFold {
    /// The node goes back to ready with a fresh method resolve.
    Retry,
    /// The failure budget is spent (or the error can never heal).
    Abandoned,
}

fn is_terminal(status: &NodeStatus) -> bool {
    matches!(status, NodeStatus::Done | NodeStatus::Abandoned)
}

fn release_active(life: &mut Life, index: usize) {
    if life.active.as_deref() == Some(life.nodes[index].id.as_str()) {
        life.active = None;
    }
}

/// Next free `n<k>` id (ids never reuse a number, even after growth).
pub fn next_id(life: &Life) -> String {
    format!("n{}", max_id_number(life) + 1)
}

fn max_id_number(life: &Life) -> usize {
    life.nodes
        .iter()
        .filter_map(|node| node.id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok()))
        .max()
        .unwrap_or(0)
        .max(life.nodes.len())
}

fn abandon(node: &mut Node, why: String, changes: &mut Vec<String>) {
    changes.push(format!("{} abandoned: {why}", node.id));
    node.status = NodeStatus::Abandoned;
    node.method = None;
    if node.last_error.is_empty() {
        node.last_error = why;
    }
}

/// Restore every invariant; returns one human line per change (for the
/// daemon log). Idempotent: a second call returns no change.
pub fn normalize(life: &mut Life) -> Vec<String> {
    let mut changes = Vec::new();
    // Legacy `failed` statuses (older files, typed launch failures) are
    // retries: the failure itself was already counted.
    for node in &mut life.nodes {
        if let NodeStatus::Failed { error } = &node.status {
            if node.last_error.is_empty() {
                node.last_error = error.clone();
            }
            changes.push(format!("{} failed -> ready", node.id));
            node.status = NodeStatus::Ready;
            node.method = None;
        }
    }
    // Active: only `life.active` may be active (locks first, so the
    // cascade below sees orphans as the open nodes they are).
    if let Some(active) = life.active.clone() {
        let live = life.nodes.iter().any(|node| node.id == active && node.status == NodeStatus::Active);
        if !live {
            changes.push(format!("active {active} is not running, cleared"));
            life.active = None;
        }
    }
    let active = life.active.clone();
    for node in &mut life.nodes {
        if node.status == NodeStatus::Active && active.as_deref() != Some(node.id.as_str()) {
            changes.push(format!("{} orphan active -> ready", node.id));
            node.status = NodeStatus::Ready;
            node.method = None;
        }
    }
    cascade_abandon(life, &mut changes);
    abandon_unresolvable(life, &mut changes);
    let done: HashSet<String> = life.nodes.iter().filter(|node| node.status == NodeStatus::Done).map(|node| node.id.clone()).collect();
    for node in &mut life.nodes {
        let deps_done = node.deps.iter().all(|dep| done.contains(dep));
        match &node.status {
            NodeStatus::Pending | NodeStatus::Blocked { .. } if deps_done => {
                changes.push(format!("{} {} -> ready", node.id, node.status.as_str()));
                node.status = NodeStatus::Ready;
            }
            NodeStatus::Ready if !deps_done => {
                changes.push(format!("{} ready with open deps -> pending", node.id));
                node.status = NodeStatus::Pending;
            }
            _ => {}
        }
    }
    changes
}

/// Abandoned (or dangling) dependencies abandon their open dependents,
/// transitively. A running node is left alone: its report decides.
fn cascade_abandon(life: &mut Life, changes: &mut Vec<String>) {
    loop {
        let known: HashSet<String> = life.nodes.iter().map(|node| node.id.clone()).collect();
        let dead: HashSet<String> =
            life.nodes.iter().filter(|node| node.status == NodeStatus::Abandoned).map(|node| node.id.clone()).collect();
        let mut moved = false;
        for node in &mut life.nodes {
            if is_terminal(&node.status) || node.status == NodeStatus::Active {
                continue;
            }
            let culprit = node.deps.iter().find(|dep| dead.contains(*dep) || !known.contains(*dep)).cloned();
            if let Some(dep) = culprit {
                let why = if known.contains(&dep) { format!("dependency {dep} abandoned") } else { format!("dependency {dep} does not exist") };
                abandon(node, why, changes);
                moved = true;
            }
        }
        if !moved {
            return;
        }
    }
}

/// Open nodes that can never finish (dependency cycle) are abandoned:
/// a node can finish when it is done, running, or all its deps can.
fn abandon_unresolvable(life: &mut Life, changes: &mut Vec<String>) {
    let mut resolvable: HashSet<String> = life
        .nodes
        .iter()
        .filter(|node| matches!(node.status, NodeStatus::Done | NodeStatus::Active))
        .map(|node| node.id.clone())
        .collect();
    loop {
        let mut grew = false;
        for node in &life.nodes {
            if resolvable.contains(&node.id) || is_terminal(&node.status) {
                continue;
            }
            if node.deps.iter().all(|dep| resolvable.contains(dep)) {
                resolvable.insert(node.id.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let mut moved = false;
    for node in &mut life.nodes {
        if !is_terminal(&node.status) && !resolvable.contains(&node.id) {
            abandon(node, "dependency cycle".to_string(), changes);
            moved = true;
        }
    }
    if moved {
        cascade_abandon(life, changes);
    }
}

/// State of a normalized graph (call `normalize` first).
pub fn graph_state(life: &Life) -> GraphState {
    if let Some(active) = &life.active {
        return GraphState::Running(active.clone());
    }
    if life.nodes.iter().any(|node| node.status == NodeStatus::Ready) {
        return GraphState::Launchable;
    }
    if life.nodes.iter().all(|node| node.status == NodeStatus::Done) {
        return GraphState::Finished;
    }
    GraphState::Stuck {
        abandoned: life.nodes.iter().filter(|node| node.status == NodeStatus::Abandoned).map(|node| node.id.clone()).collect(),
    }
}

/// Mark `index` launched: the only way a node becomes active.
pub fn mark_launched(life: &mut Life, index: usize) {
    life.nodes[index].status = NodeStatus::Active;
    life.active = Some(life.nodes[index].id.clone());
}

/// A runtime failure (`Failed`, `Sdk`, bad exit): counted, retried with a
/// fresh resolve until `MAX_FAILURES`, then abandoned.
pub fn record_failure(life: &mut Life, index: usize, error: &str) -> FailureFold {
    release_active(life, index);
    let node = &mut life.nodes[index];
    node.failures = node.failures.saturating_add(1);
    node.last_error = error.to_string();
    node.method = None;
    if node.failures >= MAX_FAILURES {
        node.status = NodeStatus::Abandoned;
        FailureFold::Abandoned
    } else {
        node.status = NodeStatus::Ready;
        FailureFold::Retry
    }
}

/// A contract error (`Usage`, no script for the method): the same
/// launch fails the same way, so the node is abandoned at once.
pub fn record_contract_error(life: &mut Life, index: usize, error: &str) {
    release_active(life, index);
    let node = &mut life.nodes[index];
    node.failures = node.failures.saturating_add(1);
    node.last_error = error.to_string();
    node.method = None;
    node.status = NodeStatus::Abandoned;
}

/// A reported level (or combat level) for a reach node. Real progress
/// clears its block count: a transformation skill blocks on its inputs
/// once per inventory load by design (cook, smelt, burn), and only a
/// block that never leads anywhere must count toward `MAX_BLOCKS`.
pub fn record_progress(life: &mut Life, index: usize, current: i64) {
    let node = &mut life.nodes[index];
    if current > node.progress_current {
        node.blocks = 0;
    }
    node.progress_current = current;
}

/// A blocked stop: the node waits on one `obtain_items` per missing
/// item, reusing an open obtain for the same item instead of stacking
/// duplicates. Past `MAX_BLOCKS` the node is abandoned.
pub fn record_block(life: &mut Life, index: usize, missing: &[String]) -> FailureFold {
    release_active(life, index);
    let blocks = life.nodes[index].blocks.saturating_add(1);
    life.nodes[index].blocks = blocks;
    life.nodes[index].last_error = format!("Blocked: missing {}", missing.join(", "));
    // The blocked method sizes the input batches (`replan::need_count`):
    // it clears once the sub-tasks are filed.
    if blocks > MAX_BLOCKS {
        life.nodes[index].method = None;
        life.nodes[index].status = NodeStatus::Abandoned;
        return FailureFold::Abandoned;
    }
    let mut reused = Vec::new();
    let mut fresh_needs = Vec::new();
    let mut fresh_nodes: Vec<Node> = Vec::new();
    for need in missing {
        // A combat level is a dependency like an item: one `reach_combat`
        // node trains it (goal style, a site safe for fresh bots), reused
        // when an open one already aims at least that high.
        if let Some(level) = need.strip_prefix("combat:").and_then(|raw| raw.parse::<u8>().ok()) {
            let open = life.nodes.iter().find(|node| {
                !is_terminal(&node.status) && matches!(&node.kind, NodeKind::ReachCombat { level: aimed } if *aimed >= level)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let id = format!("n{}", max_id_number(life) + 1 + fresh_nodes.len());
                    let mut training = Node::new(&id, NodeKind::ReachCombat { level }, Vec::new(), i64::from(level));
                    let style = crate::combat::goal_combat_style(&life.goal);
                    if let Ok(method) = crate::combat::training_method(style) {
                        training.alternatives.push(crate::model::Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 });
                    }
                    reused.push(id);
                    fresh_nodes.push(training);
                }
            }
            continue;
        }
        // PRD3 lot 4: a purchase short of coins waits on a `have_coins`
        // node (earned by the most profitable money method), reused when
        // an open one already aims at least that high.
        if let Some(amount) = need.strip_prefix("coins:").and_then(|raw| raw.parse::<i64>().ok()) {
            let open = life.nodes.iter().find(|node| {
                !is_terminal(&node.status) && matches!(&node.kind, NodeKind::HaveCoins { amount: aimed } if *aimed >= amount)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let id = format!("n{}", max_id_number(life) + 1 + fresh_nodes.len());
                    let mut earn = Node::new(&id, NodeKind::HaveCoins { amount }, Vec::new(), amount);
                    earn.alternatives.push(crate::model::Alternative { method: "gather:sheep/wool".to_string(), est_ticks: 0, est_coins: 0 });
                    reused.push(id);
                    fresh_nodes.push(earn);
                }
            }
            continue;
        }
        // A skill level a production needs: the reach chain for it,
        // reused when an open reach node already aims at least that high.
        if let Some((skill, level)) = need.strip_prefix("level:").and_then(|rest| rest.rsplit_once(':')).and_then(|(skill, raw)| raw.parse::<u8>().ok().map(|level| (skill.to_string(), level))) {
            let open = life.nodes.iter().find(|node| {
                !is_terminal(&node.status) && matches!(&node.kind, NodeKind::ReachLevel { skill: aimed_skill, level: aimed } if *aimed_skill == skill && *aimed >= level)
            });
            match open {
                Some(node) => reused.push(node.id.clone()),
                None => {
                    let mut counter = max_id_number(life) + fresh_nodes.len();
                    let mut after = String::new();
                    let chain = crate::expand::reach_chain(&skill, 1, level, &mut counter, &mut after);
                    if let Some(last) = chain.last() {
                        reused.push(last.id.clone());
                    }
                    fresh_nodes.extend(chain);
                }
            }
            continue;
        }
        let open = life.nodes.iter().find(|node| {
            !is_terminal(&node.status)
                && node.id != life.nodes[index].id
                && matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == need)
        });
        match open {
            Some(node) => reused.push(node.id.clone()),
            None => fresh_needs.push(need.clone()),
        }
    }
    life.nodes.extend(fresh_nodes);
    let mut counter = max_id_number(life);
    let fresh = replan_node(&mut life.nodes[index], &ReplanCause::Blocked, &fresh_needs, &mut counter);
    let node = &mut life.nodes[index];
    node.method = None;
    for id in reused {
        if !node.deps.contains(&id) {
            node.deps.push(id);
        }
    }
    node.status = NodeStatus::Pending;
    life.nodes.extend(fresh);
    FailureFold::Retry
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
