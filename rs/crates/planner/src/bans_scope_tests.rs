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
        memory: Default::default(),
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

/// Live fiona_park 2026-11-14: five unstrung sapphire amulets bid,
/// unfilled three times, rescued on the market, bid again. A node whose
/// own market buy went unfilled is rescued on its production legs
/// instead (never the same order again), and never twice.
#[test]
fn an_unfilled_buy_is_rescued_by_production_never_rebought() {
    let unfilled = super::market_unfilled_error(5, "unstrung_sapphire_amulet", 6000, 0, 1);
    assert_eq!(unfilled, "market order for 5 unstrung_sapphire_amulet unfilled after 6000 ticks (held 0, 1 cancelled)", "live text kept");
    let mut amulet = node("n5", obtain("unstrung_sapphire_amulet"), &[], "market:buy", &unfilled);
    amulet.alternatives.push(Alternative { method: "craft:unstrung_sapphire_amulet".to_string(), est_ticks: 0, est_coins: 0 });
    amulet.failures = 3;
    assert!(super::market_failed(&amulet), "the market itself failed");
    assert!(!super::market_failed(&node("n9", obtain("logs"), &[], "farm:logs", "Failed: no tree")), "a gather failure is no market failure");
    let mut life = burnt_chain(firemaking(), false);
    life.nodes.push(node("n6", obtain("strung_sapphire_amulet"), &["n5"], "string:unstrung_sapphire_amulet", "dependency n5 abandoned"));
    life.nodes.push(amulet);
    let at = life.nodes.iter().position(|entry| entry.id == "n5").expect("n5");
    assert_eq!(rescue_candidates(&life, "n5"), vec![at, at - 1], "the amulet, then the strung one waiting on it");
    let changes = crate::lifecycle::retarget_produce(&mut life, at);
    assert!(changes.iter().any(|line| line.contains("rescued off the market")), "{changes:?}");
    let amulet = &life.nodes[at];
    assert!(!amulet.alternatives.is_empty(), "production legs kept");
    assert!(amulet.alternatives.iter().all(|alt| !alt.method.starts_with("market:")), "never the market again: {:?}", amulet.alternatives);
    assert_eq!(amulet.failures, 0);
    assert_eq!(life.nodes[at - 1].status, NodeStatus::Pending, "the strung amulet waits on it again");
    // Abandoned again later: the history says it was rescued, the ban follows.
    life.history.push(crate::model::HistoryEntry { node: "n5".to_string(), started: 0, ended: 0, outcome: super::PRODUCE_RESCUE_TAG.to_string() });
    life.nodes[at].status = NodeStatus::Abandoned;
    assert!(rescue_candidates(&life, "n5").iter().all(|position| life.nodes[*position].id != "n5"), "rescued once only");
}

/// An item with no production leg has nothing to fall back on off the
/// market: no production rescue (the ban follows, the planner replans).
#[test]
fn no_production_leg_means_no_production_rescue() {
    let mut life = burnt_chain(firemaking(), false);
    let unfilled = super::market_unfilled_error(1, "no_such_object", 6000, 0, 0);
    life.nodes.push(node("n7", obtain("no_such_object"), &[], "market:buy", &unfilled));
    let at = life.nodes.len() - 1;
    assert!(crate::lifecycle::retarget_produce(&mut life, at).is_empty());
    assert_eq!(life.nodes[at].status, NodeStatus::Abandoned, "left abandoned");
}

/// Live 2026-11-14 (bh99tw2f): Rune Mysteries failed three times, the
/// essence quota waiting on it (`gather:mining/blankrune`) banned mining.
/// A quest failure bans no skill: the quest cools as a quest instead.
#[test]
fn a_failed_quest_bans_no_skill() {
    let mut quest = node("n416", NodeKind::CompleteQuest { quest: "runemysteries".to_string() }, &[], "quest:runemysteries", "Failed: rune mysteries");
    quest.failures = 3;
    let mut life = burnt_chain(GoalKind::Produce { item: "blankrune".to_string(), count: 200 }, false);
    life.nodes = vec![node("n415", obtain("blankrune"), &["n416"], "gather:mining/blankrune", "dependency n416 abandoned"), quest];
    assert!(banned_scope(&life, "n416").is_empty(), "got {:?}", banned_scope(&life, "n416"));
}
