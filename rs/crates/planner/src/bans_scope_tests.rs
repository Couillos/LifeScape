//! Tests of `bans_scope.rs`: a stuck item takes its dependency subtree
//! down, unless the market can sell it first.

use super::{banned_scope, market_route, rescue_candidates, tool_skills};
use crate::model::{Alternative, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn node(id: &str, kind: NodeKind, deps: &[&str], method: &str, error: &str) -> Node {
    let mut node = Node::new(id, kind, deps.iter().map(|dep| dep.to_string()).collect(), 1);
    node.alternatives.push(Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 });
    node.status = NodeStatus::Abandoned;
    node.last_error = error.to_string();
    node
}

fn obtain(item: &str) -> NodeKind {
    NodeKind::ObtainItems { item: item.to_string(), count: 1 }
}

/// Firemaking 30 → logs (chopped) → bronze axe (smithed) → bronze bars
/// (smelting failed three times): the whole chain lies abandoned.
fn burnt_chain(goal: GoalKind, ironman: bool) -> Life {
    let mut bars = node("n4", obtain("bronze_bar"), &[], "smelt:bronze_bar", "Failed: smelt failed at (3227,3255,0)");
    bars.failures = 3;
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(ironman, 0.5, 0.5, 7, Archetype::Producer),
        goal,
        nodes: vec![
            node("n1", NodeKind::ReachLevel { skill: "firemaking".to_string(), level: 30 }, &["n2"], "fire:logs", "dependency n2 abandoned"),
            node("n2", obtain("logs"), &["n3"], "farm:logs", "dependency n3 abandoned"),
            node("n3", obtain("bronze_axe"), &["n4"], "smith:bronze_axe", "dependency n4 abandoned"),
            bars,
        ],
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn firemaking() -> GoalKind {
    GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 30 }
}

/// User example: the bronze axe chain failed at the smelter; woodcutting
/// (the axe is its tool) and firemaking (it burns the chopped logs) go
/// with smithing, not just the goal's skill.
#[test]
fn a_failed_item_bans_every_skill_that_needs_it() {
    let life = burnt_chain(firemaking(), false);
    let cause = crate::bans::stuck_cause(&life).expect("a cause");
    assert_eq!(cause.node, "n4", "the smelter is the root");
    let scope = banned_scope(&life, &cause.node);
    for skill in ["firemaking", "woodcutting", "smithing"] {
        assert!(scope.contains(&skill.to_string()), "{skill} depends on the bars: {scope:?}");
    }
    assert_eq!(tool_skills("bronze_axe"), vec!["woodcutting".to_string()]);
    assert_eq!(tool_skills("feather"), vec!["fishing".to_string()], "fly fishing burns feathers");
    assert!(tool_skills("bronze_bar").is_empty());
}

/// Before any ban: the abandoned obtains on the failed path, the root
/// first; Iron never; a production quota's own item never.
#[test]
fn market_rescue_walks_up_the_failed_path() {
    let life = burnt_chain(firemaking(), false);
    let order: Vec<&str> = rescue_candidates(&life, "n4").into_iter().map(|at| life.nodes[at].id.as_str()).collect();
    assert_eq!(order, vec!["n4", "n3", "n2"], "bars, then the axe, then the logs");
    assert!(rescue_candidates(&burnt_chain(firemaking(), true), "n4").is_empty(), "Iron never trades");
    let quota = burnt_chain(GoalKind::Produce { item: "bronze_axe".to_string(), count: 1 }, false);
    let order: Vec<&str> = rescue_candidates(&quota, "n4").into_iter().map(|at| quota.nodes[at].id.as_str()).collect();
    assert!(!order.contains(&"n3"), "a quota is made, never bought: {order:?}");
    assert!(market_route(10, 100, Some(8), 30), "the ask is covered");
    assert!(market_route(1, 30, None, 30), "no ask: the reference price is covered");
    assert!(!market_route(10, 100, None, 30), "the wallet falls short");
    assert!(!market_route(1, 100, None, 0), "no price known, no route");
}

/// The rescue relaunches the root as a purchase and brings back what its
/// abandonment cascaded to; a second failure is never rescued again.
#[test]
fn a_rescued_obtain_buys_and_revives_its_dependents() {
    let mut life = burnt_chain(firemaking(), false);
    let changes = crate::lifecycle::retarget_market(&mut life, 3);
    assert!(!changes.is_empty());
    let bars = &life.nodes[3];
    assert_eq!(bars.alternatives.iter().map(|alt| alt.method.as_str()).collect::<Vec<_>>(), vec!["market:buy"]);
    assert_eq!((bars.failures, &bars.status), (0, &NodeStatus::Ready));
    for id in ["n1", "n2", "n3"] {
        let node = life.nodes.iter().find(|node| node.id == id).expect("node");
        assert_eq!(node.status, NodeStatus::Pending, "{id} waits again");
    }
    assert!(matches!(crate::lifecycle::graph_state(&life), crate::lifecycle::GraphState::Launchable));
    life.nodes[3].status = NodeStatus::Abandoned;
    assert!(rescue_candidates(&life, "n4").iter().all(|at| life.nodes[*at].id != "n4"), "rescued once only");
}
