//! Retirement of finished nodes (lifecycle, user request 2026-11-14:
//! « les tâches abandonnées ou réussies doivent disparaître des
//! dépendances du graph pour ne pas qu'il grossisse à l'infini »).
//!
//! The graph holds the live work only:
//! - a `done` node leaves the graph and every dependency list (a done
//!   dependency is a satisfied one: nothing waits on it any more);
//! - an `abandoned` node stays while it belongs to the current goal (node
//!   number at or past `memory.floor`, set by `append_goal`): the stuck
//!   spell reads it (market rescue, ban cause and scope, cooldown revive).
//!   Once a later goal is drawn it leaves too. One still listed by a kept
//!   node (a running node is left alone by the cascade) stays with it.
//!
//! What the brain still needs moves to the bounded `memory`
//! (`life_memory`): the id high-water mark (ids are never reused), the
//! quests the graph proved, lifetime counters and the last
//! `RECENT_CAP` finished nodes (goal root proof, janitor streak,
//! dashboard). The history keeps its last `HISTORY_TAIL` entries plus
//! every entry a rule still reads (latest ban per skill, latest goal
//! close, spell state, latest entry per live node and outcome).
//!
//! Pure; the brain calls `retire_finished` when it saves a life and once
//! at load (files written before the memory are pruned on first read).
//! Statuses never change here: nodes only leave.

use std::collections::{HashMap, HashSet};

use crate::life_memory::{Retired, RECORD_ERROR_MAX_CHARS};
use crate::model::{HistoryEntry, Life, Node, NodeKind, NodeStatus};

/// History entries always kept (newest).
pub const HISTORY_TAIL: usize = 200;
/// History length past which it is compacted back to the tail (plus the
/// entries a rule reads): compaction runs once per ~100 entries.
pub const HISTORY_COMPACT_AT: usize = 300;
/// Newest `goal` entries kept for the dashboard's goal history.
pub const GOAL_ENTRIES_KEPT: usize = 60;

/// What one retirement removed (log line at migration, tests).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Retirement {
    pub done: Vec<String>,
    pub abandoned: Vec<String>,
    pub history_dropped: usize,
    /// Floor inferred for a file written before the memory.
    pub floor_inferred: Option<usize>,
}

impl Retirement {
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.abandoned.is_empty() && self.history_dropped == 0 && self.floor_inferred.is_none()
    }

    pub fn describe(&self) -> String {
        let floor = self.floor_inferred.map(|floor| format!(", floor n{floor} inferred")).unwrap_or_default();
        format!("{} done and {} abandoned nodes retired, {} history entries dropped{floor}", self.done.len(), self.abandoned.len(), self.history_dropped)
    }
}

/// Number of an `n<k>` id (`None` for other ids: never retired by floor).
pub fn node_number(id: &str) -> Option<usize> {
    id.strip_prefix('n').and_then(|raw| raw.parse::<usize>().ok())
}

fn is_tutorial(node: &Node) -> bool {
    matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "tutorial")
}

/// Cheap check before a save: anything to retire or compact?
pub fn retire_due(life: &Life) -> bool {
    let floor = life.memory.floor;
    floor.is_none()
        || life.history.len() > HISTORY_COMPACT_AT
        || life.nodes.iter().any(|node| {
            node.status == NodeStatus::Done
                || (node.status == NodeStatus::Abandoned && node_number(&node.id).zip(floor).is_some_and(|(number, floor)| number < floor))
        })
}

/// Unit a goal root answers for: one per skill of a skill goal, one for
/// any other goal.
fn root_unit(goal: &crate::model::GoalKind, node: &Node) -> Option<String> {
    if let (crate::model::GoalKind::MaxSkills { skills, .. }, NodeKind::ReachLevel { skill, .. }) = (goal, &node.kind) {
        return skills.contains(skill).then(|| skill.clone());
    }
    crate::goals::serves_goal(goal, &node.kind).then(String::new)
}

/// First node number of the current goal, for a file written before the
/// memory: the smallest number among the open nodes, the current goal's
/// newest root(s) and everything they wait on. Nothing there: past every
/// node (all finished work is retired).
pub fn infer_floor(life: &Life) -> usize {
    let mut roots: HashMap<String, (usize, &Node)> = HashMap::new();
    for node in &life.nodes {
        let (Some(unit), Some(number)) = (root_unit(&life.goal, node), node_number(&node.id)) else { continue };
        if roots.get(&unit).is_none_or(|(known, _)| number > *known) {
            roots.insert(unit, (number, node));
        }
    }
    let by_id: HashMap<&str, &Node> = life.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = life
        .nodes
        .iter()
        .filter(|node| !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned))
        .map(|node| node.id.as_str())
        .chain(roots.values().map(|(_, node)| node.id.as_str()))
        .collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(node) = by_id.get(id) {
            stack.extend(node.deps.iter().map(String::as_str));
        }
    }
    seen.iter()
        .filter_map(|id| node_number(id))
        .min()
        .unwrap_or_else(|| crate::lifecycle::max_id_number(life) + 1)
}

/// Retire every finished node the rules above allow, record them in the
/// memory and compact the history. Idempotent.
pub fn retire_finished(life: &mut Life) -> Retirement {
    let mut retirement = Retirement::default();
    let floor = match life.memory.floor {
        Some(floor) => floor,
        None => {
            let floor = infer_floor(life);
            life.memory.floor = Some(floor);
            retirement.floor_inferred = Some(floor);
            floor
        }
    };
    // High-water mark first: the ids leaving now are never handed out again.
    life.memory.last_number = crate::lifecycle::max_id_number(life);
    for quest in crate::quests::quests_done_in_graph(life) {
        if !life.memory.quests_done.contains(&quest) {
            life.memory.quests_done.push(quest);
        }
    }
    let tutorial_open = !life.memory.quests_done.iter().any(|quest| quest == "tutorial");
    let active = life.active.clone();
    let status_of: HashMap<String, NodeStatus> = life.nodes.iter().map(|node| (node.id.clone(), node.status.clone())).collect();
    let mut gone: HashSet<String> = life
        .nodes
        .iter()
        .filter(|node| active.as_deref() != Some(node.id.as_str()))
        .filter(|node| match node.status {
            NodeStatus::Done => true,
            // An abandoned tutorial on the island waits for the operator.
            NodeStatus::Abandoned => node_number(&node.id).is_some_and(|number| number < floor) && !(is_tutorial(node) && tutorial_open),
            _ => false,
        })
        .map(|node| node.id.clone())
        .collect();
    // An abandoned node a kept node still lists stays with it (the
    // cascade leaves a running node alone; its report decides).
    loop {
        let back: Vec<String> = life
            .nodes
            .iter()
            .filter(|node| !gone.contains(&node.id))
            .flat_map(|node| node.deps.iter())
            .filter(|dep| gone.contains(*dep) && status_of.get(*dep) == Some(&NodeStatus::Abandoned))
            .cloned()
            .collect();
        if back.is_empty() {
            break;
        }
        for id in back {
            gone.remove(&id);
        }
    }
    if !gone.is_empty() {
        let mut records = Vec::new();
        for node in life.nodes.iter().filter(|node| gone.contains(&node.id)) {
            let outcome = if node.status == NodeStatus::Done { "done" } else { "abandoned" };
            if outcome == "done" {
                retirement.done.push(node.id.clone());
            } else {
                retirement.abandoned.push(node.id.clone());
            }
            records.push(Retired {
                id: node.id.clone(),
                kind: node.kind.clone(),
                outcome: outcome.to_string(),
                last_error: node.last_error.replace(['\n', '\r'], " ").chars().take(RECORD_ERROR_MAX_CHARS).collect(),
            });
        }
        life.nodes.retain(|node| !gone.contains(&node.id));
        for node in &mut life.nodes {
            node.deps.retain(|dep| !gone.contains(dep));
        }
        if life.engagement.as_ref().is_some_and(|engaged| gone.contains(&engaged.node)) {
            life.engagement = None;
        }
        life.memory.remember(records);
    }
    retirement.history_dropped = compact_history(life);
    retirement
}

/// Skills a `goal` history entry bans or lifts (`bans` shapes).
fn ban_skills(outcome: &str) -> Vec<String> {
    if let Some(skill) = outcome.strip_prefix(crate::bans::UNBAN_TAG) {
        return vec![skill.to_string()];
    }
    let Some(rest) = outcome.strip_prefix(crate::goals::STUCK_TAG) else { return Vec::new() };
    match rest.split_once('@') {
        Some((skill, _)) if !skill.is_empty() => vec![skill.to_string()],
        Some(_) => Vec::new(),
        None => rest.split(',').filter(|skill| !skill.is_empty()).map(str::to_string).collect(),
    }
}

/// Keep the last `HISTORY_TAIL` entries plus every entry a rule reads:
/// the newest `GOAL_ENTRIES_KEPT` goal entries, the latest ban/unban per
/// skill (`ban_states`), the latest goal close (`goal_closed`, money
/// goals), the latest non-goal entry (`spell`), and the latest entry per
/// (live node, outcome) (parked fills, stuck cause). Order is kept, so
/// every "latest entry" rule reads the same answer. Returns the count
/// dropped.
fn compact_history(life: &mut Life) -> usize {
    let total = life.history.len();
    if total <= HISTORY_COMPACT_AT {
        return 0;
    }
    let live: HashSet<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut keep = vec![false; total];
    let mut goal_entries = 0;
    let mut skills_seen: HashSet<String> = HashSet::new();
    let mut node_outcomes: HashSet<(&str, &str)> = HashSet::new();
    let mut cools_seen: HashSet<&str> = HashSet::new();
    let (mut close_seen, mut non_goal_seen, mut non_graph_seen) = (false, false, false);
    for (index, entry) in life.history.iter().enumerate().rev() {
        let mut wanted = index + HISTORY_TAIL >= total;
        if entry.node == "goal" {
            if goal_entries < GOAL_ENTRIES_KEPT {
                goal_entries += 1;
                wanted = true;
            }
            for skill in ban_skills(&entry.outcome) {
                wanted |= skills_seen.insert(skill);
            }
            if !close_seen && (entry.outcome.starts_with(crate::goals::DONE_TAG) || entry.outcome.starts_with(crate::goals::STUCK_TAG)) {
                close_seen = true;
                wanted = true;
            }
        } else if !non_goal_seen {
            non_goal_seen = true;
            wanted = true;
        }
        let bookkeeping = entry.node == "graph" || (entry.node == "goal" && entry.outcome.starts_with(crate::bans::UNBAN_TAG));
        if !non_graph_seen && !bookkeeping {
            non_graph_seen = true;
            wanted = true;
        }
        if live.contains(entry.node.as_str()) {
            wanted |= node_outcomes.insert((entry.node.as_str(), entry.outcome.as_str()));
        }
        // The latest cooldown of each key (`cooldown`, P1) outlives the
        // compaction: an active one still steers the resolve.
        if entry.node == crate::cooldown::COOL_NODE {
            let key = entry.outcome.split('|').next().unwrap_or_default();
            wanted |= cools_seen.insert(key);
        }
        keep[index] = wanted;
    }
    let history: Vec<HistoryEntry> = life.history.iter().zip(&keep).filter(|(_, kept)| **kept).map(|(entry, _)| entry.clone()).collect();
    let dropped = total - history.len();
    life.history = history;
    dropped
}

#[cfg(test)]
#[path = "lifecycle_retire_tests.rs"]
mod tests;
