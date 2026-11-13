//! Tests of the stuck-graph rules in `lifecycle.rs`: stale obtain legs,
//! cascade causes, and the cooldown revive (live 2026-11-12, bh99tw2f and
//! bbkjz2vk idle forever on "graph stuck ... operator decides").

use super::{graph_state, normalize, revive_stuck, GraphState, MAX_REVIVES, REVIVE_COOLDOWN_TICKS};
use crate::model::{Alternative, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["woodcutting".to_string()], level: 55 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn node(id: &str, kind: NodeKind, deps: &[&str], status: NodeStatus, methods: &[&str]) -> Node {
    let mut node = Node::new(id, kind, deps.iter().map(|dep| dep.to_string()).collect(), 1);
    node.status = status;
    node.alternatives = methods.iter().map(|method| Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 }).collect();
    node
}

fn obtain_kind(item: &str, count: i64) -> NodeKind {
    NodeKind::ObtainItems { item: item.to_string(), count }
}

fn by_id<'life>(life: &'life Life, id: &str) -> &'life Node {
    life.nodes.iter().find(|node| node.id == id).expect("node exists")
}

fn methods_of(life: &Life, id: &str) -> Vec<String> {
    by_id(life, id).alternatives.iter().map(|alt| alt.method.clone()).collect()
}

/// (a) bh99tw2f n1/n5: the pre-PRD3 planner froze `farm:<item>` legs in
/// the life file for any object with a drop source (imps drop wool balls,
/// dwarves drop pickaxes); no combat spot hunts either, so the leg names
/// no script. An open obtain whose legs name no executable re-derives
/// them from the obtain index: spin for the wool ball, market and the
/// D-A merchant for the (unsmithable in F2P) bronze pickaxe.
#[test]
fn stale_farm_legs_rederive_from_the_obtain_index() {
    let mut life = life_of(vec![
        node("n1", obtain_kind("ball_of_wool", 20), &[], NodeStatus::Ready, &["farm:ball_of_wool"]),
        node("n5", obtain_kind("bronze_pickaxe", 1), &[], NodeStatus::Ready, &["farm:bronze_pickaxe"]),
        node("n6", obtain_kind("feather", 10), &[], NodeStatus::Ready, &["farm:feather", "market:buy"]),
    ]);
    let changes = normalize(&mut life);
    let wool = methods_of(&life, "n1");
    assert!(wool.contains(&"spin:ball_of_wool".to_string()), "spinning makes the wool ball, got {wool:?}");
    assert!(!wool.iter().any(|method| method.starts_with("farm:")), "no dead farm leg left, got {wool:?}");
    let pickaxe = methods_of(&life, "n5");
    assert!(pickaxe.contains(&"market:buy".to_string()) && pickaxe.contains(&"shop:buy".to_string()), "market then D-A merchant, got {pickaxe:?}");
    assert!(!pickaxe.iter().any(|method| method.starts_with("farm:") || method.starts_with("smith:")), "never farmed nor smithed, got {pickaxe:?}");
    assert_eq!(methods_of(&life, "n6"), vec!["farm:feather".to_string(), "market:buy".to_string()], "live legs are left alone");
    assert!(changes.iter().any(|line| line.contains("n1") && line.contains("farm:ball_of_wool")), "the re-derivation is logged: {changes:?}");
    assert!(normalize(&mut life).is_empty(), "idempotent");
}

/// (b) A node abandoned by the cascade names the dependency, not the
/// stale block it was waiting on (bbkjz2vk n19 read "Blocked: missing
/// coins:1" although its have_coins node n20 had been abandoned).
#[test]
fn cascade_names_the_abandoned_dependency() {
    let mut blocked = node("n19", obtain_kind("hammer", 1), &["n20"], NodeStatus::Pending, &["market:buy", "shop:buy"]);
    blocked.last_error = "Blocked: missing coins:1".to_string();
    let mut earn = node("n20", NodeKind::HaveCoins { amount: 1 }, &[], NodeStatus::Abandoned, &[]);
    earn.last_error = "no report: worker went idle without reporting (crash or restart)".to_string();
    let mut life = life_of(vec![blocked, earn]);
    normalize(&mut life);
    assert_eq!(by_id(&life, "n19").status, NodeStatus::Abandoned);
    assert_eq!(by_id(&life, "n19").last_error, "dependency n20 abandoned");
}

/// bbkjz2vk's chain: woodcut 15 waits on a bronze axe, the axe (smith) on
/// a hammer, the hammer on one coin. Three report-less failures abandoned
/// the coin node and the cascade took the whole chain.
fn coin_chain(root: NodeStatus) -> Life {
    let mut root_node = node("n20", NodeKind::HaveCoins { amount: 1 }, &[], root, &["fight:melee/men"]);
    root_node.failures = 3;
    life_of(vec![
        node("n14", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 15 }, &["n18"], NodeStatus::Pending, &["gather:woodcut/logs"]),
        node("n18", obtain_kind("bronze_axe", 1), &["n19"], NodeStatus::Pending, &["market:buy", "smith:bronze_axe", "farm:bronze_axe"]),
        node("n19", obtain_kind("hammer", 1), &["n20"], NodeStatus::Pending, &["market:buy", "shop:buy"]),
        root_node,
        node("n15", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 30 }, &["n14"], NodeStatus::Pending, &["gather:woodcut/oak_logs"]),
    ])
}

/// (d) A stuck graph waits `REVIVE_COOLDOWN_TICKS`, then every abandoned
/// node is retried with fresh counters (its cascade with it); a node is
/// revived at most `MAX_REVIVES` times, then its abandonment is final and
/// the operator decides.
#[test]
fn stuck_graph_revives_after_the_cooldown_until_the_cap() {
    let mut life = coin_chain(NodeStatus::Abandoned);
    normalize(&mut life);
    assert!(matches!(graph_state(&life), GraphState::Stuck { .. }));
    assert!(revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS - 1).is_empty(), "too early: nothing moves");
    assert!(matches!(graph_state(&life), GraphState::Stuck { .. }));
    let changes = revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS);
    assert!(changes.iter().any(|line| line.contains("n20") && line.contains("revived")), "logged: {changes:?}");
    assert_eq!(graph_state(&life), GraphState::Launchable);
    assert_eq!(by_id(&life, "n20").status, NodeStatus::Ready, "the root runs again");
    assert_eq!(by_id(&life, "n20").failures, 0, "fresh failure budget");
    assert_eq!(by_id(&life, "n20").revives, 1);
    for id in ["n19", "n18", "n14", "n15"] {
        assert_eq!(by_id(&life, id).status, NodeStatus::Pending, "{id} waits on its revived chain");
    }
    // The root fails again until the cap: each cycle re-abandons, waits,
    // revives; past MAX_REVIVES the graph stays stuck for the operator.
    for _ in 1..MAX_REVIVES {
        let root = life.nodes.iter().position(|node| node.id == "n20").expect("root");
        super::record_contract_error(&mut life, root, "no-script: x");
        normalize(&mut life);
        assert!(!revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS).is_empty());
    }
    let root = life.nodes.iter().position(|node| node.id == "n20").expect("root");
    super::record_contract_error(&mut life, root, "no-script: x");
    normalize(&mut life);
    assert_eq!(by_id(&life, "n20").revives, MAX_REVIVES);
    assert!(revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS * 10).is_empty(), "the cap is final");
    assert!(matches!(graph_state(&life), GraphState::Stuck { .. }));
}

/// A revive never fires on a graph that still has work, and a revived
/// obtain re-derives its legs (bh99tw2f n1 comes back with spinning, n5
/// with the market and the merchant).
#[test]
fn revive_only_on_stuck_graphs_and_rederives_obtain_legs() {
    let mut open = coin_chain(NodeStatus::Ready);
    normalize(&mut open);
    assert!(revive_stuck(&mut open, REVIVE_COOLDOWN_TICKS).is_empty(), "a launchable graph is not stuck");
    let mut life = life_of(vec![
        node("n1", obtain_kind("ball_of_wool", 20), &[], NodeStatus::Abandoned, &["farm:ball_of_wool"]),
        node("n2", NodeKind::CompleteQuest { quest: "sheep".to_string() }, &["n1"], NodeStatus::Abandoned, &["quest:sheep"]),
        node("n24", NodeKind::ReachLevel { skill: "woodcutting".to_string(), level: 15 }, &[], NodeStatus::Abandoned, &[]),
    ]);
    normalize(&mut life);
    revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS);
    assert!(methods_of(&life, "n1").contains(&"spin:ball_of_wool".to_string()), "got {:?}", methods_of(&life, "n1"));
    assert_eq!(by_id(&life, "n2").status, NodeStatus::Pending, "the quest waits on its wool again");
    assert_eq!(methods_of(&life, "n24"), vec![crate::tranches::reach_method("woodcutting", 1)], "travel-banned tranche gets its method back");
    assert_eq!(by_id(&life, "n24").status, NodeStatus::Ready);
}

/// Cleanup janitors keep their own stop rule (two abandoned in a row stop
/// the inserts): a revive never touches them.
#[test]
fn revive_leaves_cleanup_janitors_alone() {
    let mut life = life_of(vec![node("n3", NodeKind::SpaceCleanup, &[], NodeStatus::Abandoned, &["space:cleanup"])]);
    assert!(revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS).is_empty());
    assert_eq!(by_id(&life, "n3").status, NodeStatus::Abandoned);
}
