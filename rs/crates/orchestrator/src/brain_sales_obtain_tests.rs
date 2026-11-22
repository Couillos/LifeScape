//! The inputs of an open obtain node (a market answer, a sub-task) are
//! not surplus (live 2026-11-22, owen_wood: a shop trip planned while
//! `n2875 smelt:steel_bar` was open offered 7 680 iron ore).

use crate::brain_sales::{tests::life_training, working_keep};
use lifescape_planner::{Node, NodeKind, NodeStatus};

fn obtain_node(id: &str, item: &str, method: Option<&str>, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: item.to_string(), count: 26 }, Vec::new(), 26);
    node.status = status;
    node.method = method.map(str::to_string);
    node
}

/// A market answer is an obtain node whose method smelts: its ore and
/// coal stay, its bars are sold.
#[test]
fn an_open_obtain_node_keeps_the_inputs_of_its_method() {
    let mut life = life_training(&[]);
    life.nodes.push(obtain_node("n2875", "steel_bar", Some("smelt:steel_bar"), NodeStatus::Ready));
    let keep = working_keep(&life, &[]);
    for input in ["iron_ore", "coal"] {
        assert!(keep.contains(&input.to_string()), "{input}: {keep:?}");
    }
}

/// Not resolved yet (method not drawn): the inputs of the item's own
/// production way still stay.
#[test]
fn an_unresolved_obtain_node_keeps_the_inputs_of_its_production() {
    let mut life = life_training(&[]);
    life.nodes.push(obtain_node("n2876", "bronze_bar", None, NodeStatus::Pending));
    let keep = working_keep(&life, &[]);
    for input in ["copper_ore", "tin_ore"] {
        assert!(keep.contains(&input.to_string()), "{input}: {keep:?}");
    }
}

/// A done obtain node reserves nothing.
#[test]
fn a_done_obtain_node_keeps_nothing() {
    let mut life = life_training(&[]);
    life.nodes.push(obtain_node("n2877", "steel_bar", Some("smelt:steel_bar"), NodeStatus::Done));
    let keep = working_keep(&life, &[]);
    assert!(!keep.contains(&"iron_ore".to_string()), "{keep:?}");
}
