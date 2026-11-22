//! Open work of earlier goals (live 2026-11-22 wolfe_carl: a side goal
//! closed stuck at 17:28 left 34 open `obtain uncut_sapphire` nodes; the
//! launch pass kept picking them 16 hours after the goal was given back,
//! and the stuck redraws had stacked them). A goal's open nodes below
//! `memory.floor` that nothing of the current goal waits on are no work of
//! the bot any more: they leave as abandoned (the retirement drops them).
//! Spared, like `abandon_goal_work`: the running node, a buy really posted
//! on the book (`earlier_wait` ignores it), the janitor, the gear ladder;
//! and everything an open node of the current goal (or a spared one)
//! depends on, transitively.

use std::collections::HashSet;

use crate::lifecycle_retire::node_number;
use crate::model::{Life, NodeKind, NodeStatus};

/// Abandons the stale open nodes; returns their ids (empty: nothing to do).
pub fn abandon_stale_goal_work(life: &mut Life, cause: &str) -> Vec<String> {
    let Some(floor) = life.memory.floor else { return Vec::new() };
    let is_open = |status: &NodeStatus| !matches!(status, NodeStatus::Done | NodeStatus::Abandoned);
    let stale_number = |id: &str| node_number(id).is_some_and(|number| number < floor);
    if !life.nodes.iter().any(|node| is_open(&node.status) && stale_number(&node.id)) {
        return Vec::new();
    }
    let gear: HashSet<String> = crate::gear_ladder::gear_subtree(life).into_iter().collect();
    let posted: HashSet<&str> = life
        .nodes
        .iter()
        .filter(|node| node.method.as_deref().is_some_and(|method| method.starts_with("market:")))
        .filter(|node| life.history.iter().rev().find(|entry| entry.node == node.id).is_some_and(|entry| entry.outcome == "awaiting_fill"))
        .map(|node| node.id.as_str())
        .collect();
    let mut stack: Vec<&str> = life
        .nodes
        .iter()
        .filter(|node| is_open(&node.status))
        .filter(|node| {
            !stale_number(&node.id)
                || node.status == NodeStatus::Active
                || life.active.as_deref() == Some(node.id.as_str())
                || posted.contains(node.id.as_str())
                || gear.contains(&node.id)
                || matches!(node.kind, NodeKind::SpaceCleanup)
        })
        .map(|node| node.id.as_str())
        .collect();
    let mut kept: HashSet<&str> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !kept.insert(id) {
            continue;
        }
        if let Some(node) = life.nodes.iter().find(|node| node.id == id) {
            stack.extend(node.deps.iter().map(String::as_str));
        }
    }
    let stale: Vec<String> = life.nodes.iter().filter(|node| is_open(&node.status) && stale_number(&node.id) && !kept.contains(node.id.as_str())).map(|node| node.id.clone()).collect();
    for node in life.nodes.iter_mut().filter(|node| stale.contains(&node.id)) {
        node.status = NodeStatus::Abandoned;
        node.last_error = cause.to_string();
    }
    if life.engagement.as_ref().is_some_and(|engaged| stale.contains(&engaged.node)) {
        life.engagement = None;
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::abandon_stale_goal_work;
    use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

    fn obtain(id: &str, status: NodeStatus, deps: &[&str]) -> Node {
        let mut node = Node::new(id, NodeKind::ObtainItems { item: format!("item_{id}"), count: 5 }, deps.iter().map(|dep| dep.to_string()).collect(), 5);
        node.status = status;
        node
    }

    fn life_of(nodes: Vec<Node>, floor: usize) -> Life {
        let mut life = Life {
            schema: 1,
            account: "t00".to_string(),
            created_at: 0,
            seed: 7,
            profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
            goal: GoalKind::MakeMoney,
            nodes,
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        life.memory.floor = Some(floor);
        life
    }

    /// Stale ready and pending nodes of a closed goal leave; a stale node
    /// the current goal waits on, the running node and the current goal's
    /// own nodes stay.
    #[test]
    fn open_nodes_of_earlier_goals_leave_unless_the_current_goal_needs_them() {
        let mut life = life_of(
            vec![
                obtain("n1", NodeStatus::Ready, &[]),
                obtain("n2", NodeStatus::Pending, &["n1"]),
                obtain("n3", NodeStatus::Ready, &[]),
                obtain("n4", NodeStatus::Ready, &[]),
                obtain("n10", NodeStatus::Pending, &["n3"]),
                obtain("n11", NodeStatus::Ready, &[]),
            ],
            10,
        );
        life.nodes[3].status = NodeStatus::Active;
        life.active = Some("n4".to_string());
        let gone = abandon_stale_goal_work(&mut life, "stale");
        assert_eq!(gone, vec!["n1".to_string(), "n2".to_string()]);
        let status = |id: &str| life.nodes.iter().find(|node| node.id == id).map(|node| node.status.clone()).expect("node");
        assert_eq!(status("n3"), NodeStatus::Ready, "n10 waits on it");
        assert_eq!(status("n4"), NodeStatus::Active);
        assert_eq!(status("n11"), NodeStatus::Ready);
        assert!(abandon_stale_goal_work(&mut life, "stale").is_empty(), "idempotent");
    }

    /// A buy left on the book (`awaiting_fill`) stays: `earlier_wait`
    /// already ignores it.
    #[test]
    fn a_posted_buy_is_spared() {
        let mut buy = obtain("n1", NodeStatus::Ready, &[]);
        buy.method = Some("market:buy".to_string());
        let mut life = life_of(vec![buy], 10);
        life.history.push(crate::model::HistoryEntry { node: "n1".to_string(), started: 1, ended: 1, outcome: "awaiting_fill".to_string() });
        assert!(abandon_stale_goal_work(&mut life, "stale").is_empty());
    }
}
