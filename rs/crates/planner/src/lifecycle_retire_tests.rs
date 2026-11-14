//! Tests of `lifecycle_retire.rs` (user request 2026-11-14: finished
//! tasks leave the graph, the life file stays bounded).

use super::{infer_floor, retire_due, retire_finished, GOAL_ENTRIES_KEPT, HISTORY_COMPACT_AT, HISTORY_TAIL};
use crate::lifecycle::{complete_node, graph_state, mark_launched, next_id, normalize, record_block, record_failure, revive_stuck, GraphState, MAX_FAILURES, REVIVE_COOLDOWN_TICKS};
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn node(id: &str, kind: NodeKind, deps: &[&str], status: NodeStatus) -> Node {
    let mut node = Node::new(id, kind, deps.iter().map(|dep| dep.to_string()).collect(), 1);
    node.status = status;
    node
}

fn reach(id: &str, skill: &str, level: u8, deps: &[&str], status: NodeStatus) -> Node {
    node(id, NodeKind::ReachLevel { skill: skill.to_string(), level }, deps, status)
}

fn obtain(id: &str, item: &str, deps: &[&str], status: NodeStatus) -> Node {
    node(id, NodeKind::ObtainItems { item: item.to_string(), count: 1 }, deps, status)
}

fn entry(node: &str, outcome: &str, at: i64) -> HistoryEntry {
    HistoryEntry { node: node.to_string(), started: at, ended: at, outcome: outcome.to_string() }
}

fn life_of(goal: GoalKind, nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal,
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn mining(level: u8) -> GoalKind {
    GoalKind::MaxSkills { skills: vec!["mining".to_string()], level }
}

fn ids(life: &Life) -> Vec<&str> {
    life.nodes.iter().map(|node| node.id.as_str()).collect()
}

/// A done node leaves the graph and every dependency list: its dependent
/// is ready on its other deps alone, the record keeps what it was.
#[test]
fn a_done_node_leaves_the_graph_and_the_dependency_lists() {
    let mut life = life_of(
        mining(20),
        vec![obtain("n1", "bronze_pickaxe", &[], NodeStatus::Done), obtain("n2", "tin_ore", &[], NodeStatus::Ready), reach("n3", "mining", 20, &["n1", "n2"], NodeStatus::Pending)],
    );
    let gone = retire_finished(&mut life);
    assert_eq!(gone.done, vec!["n1".to_string()]);
    assert_eq!(ids(&life), vec!["n2", "n3"]);
    assert_eq!(life.nodes[1].deps, vec!["n2".to_string()], "the done dependency is satisfied, not listed");
    assert_eq!(life.memory.recent.last().map(|row| (row.id.as_str(), row.outcome.as_str())), Some(("n1", "done")));
    assert_eq!(life.memory.done_total, 1);
    normalize(&mut life);
    assert_eq!(graph_state(&life), GraphState::Launchable);
    // Once n2 completes too, n3 waits on nothing and runs.
    complete_node(&mut life, 0, 1);
    normalize(&mut life);
    retire_finished(&mut life);
    normalize(&mut life);
    assert_eq!(ids(&life), vec!["n3"]);
    assert!(life.nodes[0].deps.is_empty());
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert_eq!(retire_finished(&mut life), super::Retirement::default(), "idempotent");
}

/// The current goal's abandoned nodes stay (rescue, ban cause, revive);
/// a later goal retires them, the cascade with them.
#[test]
fn abandoned_nodes_stay_with_their_goal_and_leave_with_it() {
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    let mut life = life_of(mining(20), Vec::new());
    crate::goals::append_goal(&mut life, mining(20), &profile, &|_| 10, &|_| 0);
    let first_goal: Vec<String> = life.nodes.iter().map(|node| node.id.clone()).collect();
    assert!(!first_goal.is_empty());
    life.nodes[0].status = NodeStatus::Ready;
    mark_launched(&mut life, 0);
    for _ in 0..MAX_FAILURES {
        if life.nodes[0].status != NodeStatus::Active {
            mark_launched(&mut life, 0);
        }
        record_failure(&mut life, 0, "Failed: no rocks");
    }
    normalize(&mut life);
    assert!(matches!(graph_state(&life), GraphState::Stuck { .. }));
    retire_finished(&mut life);
    assert_eq!(life.nodes.len(), first_goal.len(), "the stuck goal keeps its abandoned nodes");
    assert!(crate::bans::stuck_cause(&life).is_some_and(|cause| cause.last_error == "Failed: no rocks"), "the ban reads its cause");
    assert!(!revive_stuck(&mut life, REVIVE_COOLDOWN_TICKS).is_empty(), "the cooldown revive finds them");
    for node in &mut life.nodes {
        node.status = NodeStatus::Abandoned;
    }
    crate::goals::append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 }, &profile, &|_| 10, &|_| 0);
    let gone = retire_finished(&mut life);
    assert_eq!(gone.abandoned, first_goal, "the closed goal's abandons retire");
    assert!(life.nodes.iter().all(|node| node.status != NodeStatus::Abandoned));
    normalize(&mut life);
    assert_eq!(graph_state(&life), GraphState::Launchable);
}

/// Ids are never reused once their node retired, and a late report for
/// a retired id finds no node.
#[test]
fn retired_ids_are_never_handed_out_again() {
    let mut life = life_of(mining(20), vec![reach("n1", "mining", 15, &[], NodeStatus::Done), reach("n7", "mining", 20, &[], NodeStatus::Done)]);
    life.history.push(entry("n7", "goal_reached", 1));
    retire_finished(&mut life);
    life.history.clear();
    assert!(life.nodes.is_empty());
    assert_eq!(life.memory.last_number, 7);
    assert_eq!(next_id(&life), "n8", "past the retired n7");
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    crate::goals::append_goal(&mut life, GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 }, &profile, &|_| 10, &|_| 0);
    assert!(life.nodes.iter().all(|node| super::node_number(&node.id).is_some_and(|number| number > 7)), "{:?}", ids(&life));
    assert_eq!(life.memory.floor, Some(8));
    // A block files sub-tasks past the mark too.
    let blocked = life.nodes.len() - 1;
    record_block(&mut life, blocked, &["fishing_rod".to_string()]);
    let mut numbers: Vec<usize> = life.nodes.iter().filter_map(|node| super::node_number(&node.id)).collect();
    numbers.sort();
    numbers.dedup();
    assert_eq!(numbers.len(), life.nodes.len(), "unique ids");
}

/// The goal's root proof survives its retirement (equip step done, the
/// graph empty): the goal reads reached, never stuck.
#[test]
fn a_retired_done_root_still_proves_the_goal() {
    let goal = GoalKind::EquipBest { style: "melee".to_string() };
    let mut life = life_of(goal.clone(), vec![obtain("n3", "leather_boots", &[], NodeStatus::Abandoned), node("n5", NodeKind::EquipBest { style: "melee".to_string() }, &[], NodeStatus::Done)]);
    life.memory.floor = Some(3);
    retire_finished(&mut life);
    assert_eq!(ids(&life), vec!["n3"]);
    assert!(matches!(graph_state(&life), GraphState::Stuck { .. }));
    assert!(crate::goals::goal_root_done(&life), "the retired root is done");
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    crate::goals::append_goal(&mut life, goal, &profile, &|_| 10, &|_| 0);
    assert!(!crate::goals::goal_root_done(&life), "a newer root decides");
}

/// Quest proofs and the tutorial outlive their nodes; a block on the
/// quest disproves the retired proof as it did the node.
#[test]
fn quest_proofs_outlive_their_nodes() {
    let quest = |id: &str, key: &str, status: NodeStatus| node(id, NodeKind::CompleteQuest { quest: key.to_string() }, &[], status);
    let mut life = life_of(mining(20), vec![quest("n1", "tutorial", NodeStatus::Done), quest("n2", "rune_mysteries", NodeStatus::Done), reach("n3", "mining", 20, &[], NodeStatus::Ready)]);
    retire_finished(&mut life);
    assert_eq!(ids(&life), vec!["n3"]);
    let mut done = crate::quests::quests_done_in_graph(&life);
    done.sort();
    assert_eq!(done, vec!["rune_mysteries".to_string(), "tutorial".to_string()]);
    record_block(&mut life, 0, &["quest:rune_mysteries".to_string()]);
    assert_eq!(crate::quests::quests_done_in_graph(&life), vec!["tutorial".to_string()], "disproved");
}

/// An abandoned tutorial on the island waits for the operator: never
/// retired while the tutorial is not proven.
#[test]
fn an_abandoned_tutorial_waits_for_the_operator() {
    let mut life = life_of(mining(20), vec![node("n1", NodeKind::CompleteQuest { quest: "tutorial".to_string() }, &[], NodeStatus::Abandoned), reach("n9", "mining", 20, &[], NodeStatus::Ready)]);
    life.memory.floor = Some(5);
    retire_finished(&mut life);
    assert_eq!(ids(&life), vec!["n1", "n9"]);
}

/// A running node's abandoned dependency stays with it (the cascade
/// leaves a running node alone).
#[test]
fn a_running_node_keeps_its_abandoned_dependency() {
    let mut life = life_of(mining(20), vec![obtain("n1", "tin_ore", &[], NodeStatus::Abandoned), reach("n2", "mining", 20, &["n1"], NodeStatus::Active)]);
    life.active = Some("n2".to_string());
    life.memory.floor = Some(10);
    retire_finished(&mut life);
    assert_eq!(ids(&life), vec!["n1", "n2"]);
}

/// Janitor streak across retirement: two abandoned cleanups in a row
/// still stall, wherever they are kept.
#[test]
fn the_janitor_streak_reads_retired_cleanups() {
    let mut life = life_of(mining(20), vec![node("n1", NodeKind::SpaceCleanup, &[], NodeStatus::Abandoned), node("n2", NodeKind::SpaceCleanup, &[], NodeStatus::Abandoned)]);
    life.memory.floor = Some(5);
    assert!(crate::space::janitor_stalled(&life));
    retire_finished(&mut life);
    assert!(life.nodes.is_empty());
    assert!(crate::space::janitor_stalled(&life), "the records keep the streak");
    life.nodes.push(node("n3", NodeKind::SpaceCleanup, &[], NodeStatus::Done));
    assert!(!crate::space::janitor_stalled(&life), "a done cleanup breaks it");
}

/// A file written before the memory: the floor is the current goal's
/// work (open nodes, the goal's newest roots and what they wait on).
#[test]
fn the_floor_of_an_old_file_is_the_current_goal() {
    let mut nodes = Vec::new();
    for number in 1..=90 {
        nodes.push(obtain(&format!("n{number}"), "logs", &[], if number % 3 == 0 { NodeStatus::Abandoned } else { NodeStatus::Done }));
    }
    nodes.push(obtain("n91", "tin_ore", &[], NodeStatus::Abandoned));
    nodes.push(reach("n92", "mining", 15, &["n91"], NodeStatus::Abandoned));
    nodes.push(reach("n93", "mining", 20, &["n92"], NodeStatus::Abandoned));
    nodes.push(node("n94", NodeKind::SpaceCleanup, &[], NodeStatus::Ready));
    let mut life = life_of(mining(20), nodes);
    assert_eq!(infer_floor(&life), 91);
    let gone = retire_finished(&mut life);
    assert_eq!(gone.floor_inferred, Some(91));
    assert_eq!(ids(&life), vec!["n91", "n92", "n93", "n94"], "the stuck goal and the open janitor stay");
    assert_eq!(gone.done.len() + gone.abandoned.len(), 90);
    // Everything finished and no root: past every node.
    let mut finished = life_of(GoalKind::MakeMoney, vec![obtain("n4", "logs", &[], NodeStatus::Abandoned)]);
    assert_eq!(infer_floor(&finished), 5);
    retire_finished(&mut finished);
    assert!(finished.nodes.is_empty());
}

/// The history keeps its tail and every entry a rule reads: ban states,
/// goal close, spell, the latest entry per live node.
#[test]
fn history_compaction_keeps_what_the_rules_read() {
    let mut life = life_of(mining(20), vec![obtain("n500", "tin_ore", &[], NodeStatus::Ready)]);
    let cause = crate::bans::StuckCause { node: "n3".to_string(), class: "3 failures".to_string(), last_error: "x".to_string() };
    life.history.push(entry("goal", &crate::bans::ban_entry("fishing", 6000, &cause), 10));
    life.history.push(entry("goal", "stuck:woodcutting,firemaking", 11));
    life.history.push(entry("goal", "unban:firemaking", 12));
    life.history.push(entry("n500", "awaiting_fill", 13));
    life.history.push(entry("goal", "done:mining", 14));
    for at in 0..2000 {
        life.history.push(entry(&format!("n{}", at % 40), "launched", 100 + at));
    }
    life.history.push(entry("graph", "stuck", 5000));
    let since = |at: i64| (100 - at).max(0) as u64;
    let bans_before = crate::bans::ban_states(&life.history, &since);
    let closed_before = crate::goals::goal_closed(&life.history);
    assert!(retire_due(&life));
    let gone = retire_finished(&mut life);
    assert!(life.history.len() <= HISTORY_TAIL + GOAL_ENTRIES_KEPT + 10, "bounded: {}", life.history.len());
    assert!(gone.history_dropped > 1500);
    assert_eq!(crate::bans::ban_states(&life.history, &since), bans_before, "same bans");
    assert_eq!(crate::goals::goal_closed(&life.history), closed_before);
    assert!(life.history.iter().any(|row| row.node == "n500" && row.outcome == "awaiting_fill"), "the parked fill keeps its stamp");
    assert_eq!(life.history.last().map(|row| row.node.as_str()), Some("graph"));
    let len = life.history.len();
    assert!(len < HISTORY_COMPACT_AT);
    assert_eq!(retire_finished(&mut life).history_dropped, 0, "idempotent below the threshold");
}

/// Hundreds of goals later the life is the size of one goal: nodes,
/// history and the serialized file stay bounded.
#[test]
fn a_long_life_stays_bounded() {
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    let mut life = life_of(mining(20), Vec::new());
    let mut largest = 0;
    for round in 0..150u32 {
        let goal = if round % 2 == 0 { mining(20) } else { GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 20 } };
        crate::goals::append_goal(&mut life, goal, &profile, &|_| 10, &|_| 0);
        normalize(&mut life);
        for (index, node) in life.nodes.clone().iter().enumerate() {
            if node.status == NodeStatus::Ready {
                mark_launched(&mut life, index);
                if round % 3 == 0 {
                    crate::lifecycle::record_contract_error(&mut life, index, "Usage: x");
                } else {
                    complete_node(&mut life, index, 1);
                }
                life.history.push(entry(&node.id, "launched", i64::from(round)));
            }
        }
        normalize(&mut life);
        life.history.push(entry("goal", "next:mining 20", i64::from(round)));
        retire_finished(&mut life);
        largest = largest.max(lifescape_json::stringify(&crate::model::life_to_value(&life)).len());
    }
    assert!(life.nodes.len() <= 10, "nodes: {}", life.nodes.len());
    assert!(life.history.len() <= HISTORY_COMPACT_AT, "history: {}", life.history.len());
    assert!(largest < 80_000, "file stays small: {largest} bytes");
    assert!(life.memory.done_total + life.memory.abandoned_total > 150, "the counters keep the totals");
    let back = crate::model::life_from_value(&crate::model::life_to_value(&life)).expect("round trip");
    assert_eq!(back, life);
}
