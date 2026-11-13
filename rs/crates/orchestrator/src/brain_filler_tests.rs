//! Market waits never idle the worker (user rule 2026-11-13).

use super::parked_waits;
use lifescape_planner::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn obtain(id: &str, method: Option<&str>, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: "amulet_of_power".to_string(), count: 1 }, Vec::new(), 1);
    node.method = method.map(str::to_string);
    node.status = status;
    node
}

fn life_of(nodes: Vec<Node>, history: Vec<HistoryEntry>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MakeMoney,
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: None,
    }
}

/// A parked wait is a ready `market:buy` obtain whose own last history
/// entry is `awaiting_fill`; a later entry (filled, failed, partial) or
/// another method ends it.
#[test]
fn parked_waits_read_the_node_history() {
    let life = life_of(
        vec![
            obtain("n1", Some("market:buy"), NodeStatus::Ready),
            obtain("n2", Some("market:buy"), NodeStatus::Ready),
            obtain("n3", Some("smith:bronze_bar"), NodeStatus::Ready),
            obtain("n4", Some("market:buy"), NodeStatus::Done),
        ],
        vec![
            entry("n1", "awaiting_fill", 100),
            entry("n2", "awaiting_fill", 100),
            entry("n2", "partial_fill", 150),
            entry("n3", "awaiting_fill", 100),
            entry("n4", "awaiting_fill", 100),
            entry("n9", "launched", 180),
        ],
    );
    assert_eq!(parked_waits(&life), vec![(0, "n1".to_string(), 100)]);
}
