//! Tests of `lifecycle.rs` (invariants, counters, liveness).

use super::{graph_state, mark_launched, normalize, record_block, record_contract_error, record_failure, FailureFold, GraphState, MAX_FAILURES};
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn reach(id: &str, deps: &[&str], status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ReachLevel { skill: "fishing".to_string(), level: 10 }, deps.iter().map(|dep| dep.to_string()).collect(), 10);
    node.status = status;
    node
}

fn obtain(id: &str, item: &str, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: item.to_string(), count: 1 }, Vec::new(), 1);
    node.status = status;
    node
}

fn life_of(nodes: Vec<Node>, active: Option<&str>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["fishing".to_string()], level: 40 },
        nodes,
        engagement: None,
        active: active.map(str::to_string),
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn status_of(life: &Life, id: &str) -> NodeStatus {
    life.nodes.iter().find(|node| node.id == id).map(|node| node.status.clone()).expect("node exists")
}

/// The graph left by the old `revive_dead_graph` (b7e2ggbp, t02…): root
/// abandoned, dependents `ready` with deps never done, nothing active.
/// Normalized, the whole chain is abandoned and the graph reads stuck,
/// never "open with nothing to launch".
#[test]
fn revived_dead_graph_reads_stuck_not_open() {
    let mut life = life_of(
        vec![
            reach("n1", &[], NodeStatus::Abandoned),
            reach("n2", &["n1"], NodeStatus::Ready),
            reach("n3", &["n2"], NodeStatus::Ready),
            reach("n4", &["n3"], NodeStatus::Pending),
        ],
        None,
    );
    let changes = normalize(&mut life);
    assert!(!changes.is_empty());
    for id in ["n2", "n3", "n4"] {
        assert_eq!(status_of(&life, id), NodeStatus::Abandoned, "{id} cascades");
    }
    assert_eq!(graph_state(&life), GraphState::Stuck { abandoned: vec!["n1".into(), "n2".into(), "n3".into(), "n4".into()] });
    assert!(normalize(&mut life).is_empty(), "idempotent");
}

/// t48: `active` names a node, but dependents point at it while the
/// node itself is not active (stale lock): cleared, the dep chain
/// becomes launchable again.
#[test]
fn stale_active_lock_clears_and_chain_launches() {
    let mut life = life_of(vec![reach("n1", &["n11"], NodeStatus::Pending), obtain("n11", "small_fishing_net", NodeStatus::Ready)], Some("n11"));
    normalize(&mut life);
    assert_eq!(life.active, None, "a lock on a non-active node is cleared");
    assert_eq!(graph_state(&life), GraphState::Launchable);
    assert_eq!(status_of(&life, "n11"), NodeStatus::Ready);
    assert_eq!(status_of(&life, "n1"), NodeStatus::Pending, "waits on n11");
}

/// An `Active` node that is not `life.active` (tutorial overwrote the
/// lock: bt7ktb8c) goes back to ready instead of being orphaned forever.
#[test]
fn orphan_active_goes_back_to_ready() {
    let mut life = life_of(vec![reach("n1", &[], NodeStatus::Active), reach("n2", &[], NodeStatus::Active)], Some("n2"));
    normalize(&mut life);
    assert_eq!(status_of(&life, "n1"), NodeStatus::Ready);
    assert_eq!(status_of(&life, "n2"), NodeStatus::Active);
    assert_eq!(graph_state(&life), GraphState::Running("n2".to_string()));
}

#[test]
fn dangling_dependency_and_cycle_abandon() {
    let mut life = life_of(
        vec![
            reach("n1", &["n9"], NodeStatus::Pending),
            reach("n2", &["n3"], NodeStatus::Pending),
            reach("n3", &["n2"], NodeStatus::Pending),
            reach("n4", &["n3"], NodeStatus::Pending),
            reach("n5", &[], NodeStatus::Pending),
        ],
        None,
    );
    normalize(&mut life);
    assert_eq!(status_of(&life, "n1"), NodeStatus::Abandoned, "n9 does not exist");
    assert_eq!(status_of(&life, "n2"), NodeStatus::Abandoned, "cycle");
    assert_eq!(status_of(&life, "n3"), NodeStatus::Abandoned, "cycle");
    assert_eq!(status_of(&life, "n4"), NodeStatus::Abandoned, "depends on the cycle");
    assert_eq!(status_of(&life, "n5"), NodeStatus::Ready, "independent node survives");
}

#[test]
fn ready_with_open_dep_pends_and_legacy_failed_retries() {
    let mut life = life_of(
        vec![reach("n1", &[], NodeStatus::Failed { error: "no-script:x".to_string() }), reach("n2", &["n1"], NodeStatus::Ready)],
        None,
    );
    normalize(&mut life);
    assert_eq!(status_of(&life, "n1"), NodeStatus::Ready);
    assert_eq!(life.nodes[0].last_error, "no-script:x", "the cause survives");
    assert_eq!(status_of(&life, "n2"), NodeStatus::Pending);
}

#[test]
fn failures_are_bounded_and_never_reset() {
    let mut life = life_of(vec![reach("n1", &[], NodeStatus::Ready)], None);
    for attempt in 1..=MAX_FAILURES {
        mark_launched(&mut life, 0);
        let fold = record_failure(&mut life, 0, "Failed: fish failed at (0, 3086, 3227)");
        assert_eq!(life.active, None, "a failure releases the lock");
        if attempt < MAX_FAILURES {
            assert_eq!(fold, FailureFold::Retry);
            assert_eq!(status_of(&life, "n1"), NodeStatus::Ready);
        } else {
            assert_eq!(fold, FailureFold::Abandoned);
        }
        normalize(&mut life);
    }
    assert_eq!(life.nodes[0].failures, MAX_FAILURES);
    assert_eq!(graph_state(&life), GraphState::Stuck { abandoned: vec!["n1".to_string()] });
}

#[test]
fn contract_error_abandons_at_once() {
    let mut life = life_of(vec![reach("n1", &[], NodeStatus::Active)], Some("n1"));
    record_contract_error(&mut life, 0, "Usage: fishing [--level SKILL N]");
    assert_eq!(status_of(&life, "n1"), NodeStatus::Abandoned);
    assert_eq!(life.active, None);
}

/// t18/t09: a second block on the same item reuses the open obtain
/// instead of stacking a duplicate sub-task.
#[test]
fn blocks_reuse_open_obtain_and_are_bounded() {
    let mut life = life_of(vec![reach("n1", &[], NodeStatus::Active), reach("n2", &[], NodeStatus::Active)], Some("n1"));
    record_block(&mut life, 0, &["small_fishing_net".to_string()]);
    normalize(&mut life);
    let obtains = |life: &Life| life.nodes.iter().filter(|node| matches!(&node.kind, NodeKind::ObtainItems { .. })).count();
    assert_eq!(obtains(&life), 1);
    let obtain_id = life.nodes.last().map(|node| node.id.clone()).expect("obtain created");
    assert_eq!(life.nodes[0].deps, vec![obtain_id.clone()]);
    life.active = Some("n2".to_string());
    life.nodes[1].status = NodeStatus::Active;
    record_block(&mut life, 1, &["small_fishing_net".to_string()]);
    normalize(&mut life);
    assert_eq!(obtains(&life), 1, "no duplicate obtain");
    assert_eq!(life.nodes[1].deps, vec![obtain_id]);
    for _ in 0..3 {
        record_block(&mut life, 0, &["small_fishing_net".to_string()]);
    }
    assert_eq!(status_of(&life, "n1"), NodeStatus::Abandoned, "fourth block abandons");
}

/// Liveness on random graphs: after `normalize`, an unfinished graph has
/// an active or a ready node, and a second pass changes nothing.
#[test]
fn normalize_guarantees_liveness_on_random_graphs() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut draw = |bound: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % bound
    };
    for _ in 0..2000 {
        let count = 1 + draw(9) as usize;
        let mut nodes = Vec::new();
        for index in 0..count {
            let status = match draw(7) {
                0 => NodeStatus::Pending,
                1 => NodeStatus::Ready,
                2 => NodeStatus::Active,
                3 => NodeStatus::Done,
                4 => NodeStatus::Abandoned,
                5 => NodeStatus::Blocked { reason: "missing:net".to_string() },
                _ => NodeStatus::Failed { error: "x".to_string() },
            };
            let deps: Vec<String> = (0..draw(3)).map(|_| format!("n{}", 1 + draw(count as u64 + 1))).collect();
            let mut node = reach(&format!("n{}", index + 1), &[], status);
            node.deps = deps;
            nodes.push(node);
        }
        let active = if draw(2) == 0 { Some(format!("n{}", 1 + draw(count as u64))) } else { None };
        let mut life = life_of(nodes, active.as_deref());
        normalize(&mut life);
        assert!(normalize(&mut life).is_empty(), "idempotent on {:?}", life.nodes);
        let unfinished = life.nodes.iter().any(|node| !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned));
        if unfinished {
            assert!(
                life.active.is_some() || life.nodes.iter().any(|node| node.status == NodeStatus::Ready),
                "unfinished graph with nothing to launch: {:?} active={:?}",
                life.nodes,
                life.active
            );
        }
        let actives = life.nodes.iter().filter(|node| node.status == NodeStatus::Active).count();
        assert!(actives <= 1, "one active at most");
        if let Some(id) = &life.active {
            assert_eq!(status_of(&life, id), NodeStatus::Active);
        }
    }
}

/// A combat requirement becomes a `reach_combat` dependency (the site
/// pick asked for it), reused by the next node that needs as much.
#[test]
fn combat_block_adds_a_training_dependency() {
    let mut life = life_of(vec![reach("n1", &[], NodeStatus::Active), reach("n2", &[], NodeStatus::Ready)], Some("n1"));
    record_block(&mut life, 0, &["combat:15".to_string()]);
    normalize(&mut life);
    let training = life.nodes.iter().find(|node| matches!(node.kind, NodeKind::ReachCombat { level: 15 })).expect("training node");
    assert_eq!(training.status, NodeStatus::Ready, "the training runs first");
    assert!(!training.alternatives.is_empty(), "it carries its training method");
    let training_id = training.id.clone();
    assert_eq!(life.nodes[0].deps, vec![training_id.clone()]);
    assert_eq!(life.nodes[0].status, NodeStatus::Pending);
    record_block(&mut life, 1, &["combat:10".to_string()]);
    assert_eq!(life.nodes[1].deps, vec![training_id], "a 15 training covers a 10 need");
}

/// PRD3 lot 4: a coin shortfall files one `have_coins` node, a skill gap
/// files the reach chain; both are reused, never stacked.
#[test]
fn coin_and_level_blocks_file_earning_and_training() {
    let mut life = life_of(vec![obtain("n1", "bronze_sword", NodeStatus::Ready), obtain("n2", "steel_platebody", NodeStatus::Ready)], None);
    record_block(&mut life, 0, &["coins:500".to_string()]);
    let earn: Vec<&Node> = life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::HaveCoins { amount: 500 })).collect();
    assert_eq!(earn.len(), 1, "one have_coins node");
    assert!(life.nodes[0].deps.contains(&earn[0].id), "the obtain waits on the coins");
    record_block(&mut life, 1, &["coins:200".to_string(), "level:smithing:18".to_string()]);
    assert_eq!(life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::HaveCoins { .. })).count(), 1, "500 covers 200: reused");
    let reach: Vec<&Node> = life.nodes.iter().filter(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, .. } if skill == "smithing")).collect();
    assert!(reach.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { level: 18, .. })), "chain reaches 18");
    let top = reach.iter().find(|node| matches!(&node.kind, NodeKind::ReachLevel { level: 18, .. })).expect("top tranche");
    assert!(life.nodes[1].deps.contains(&top.id), "the plate waits on smithing 18");
    let mut ids: Vec<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
    ids.sort();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "unique ids");
}

/// A cook blocks on raw fish once per inventory load: level progress
/// clears the count, so only blocks that lead nowhere abandon the node.
#[test]
fn progress_clears_the_block_count() {
    let mut cook = reach("n1", &[], NodeStatus::Active);
    cook.kind = NodeKind::ReachLevel { skill: "cooking".to_string(), level: 30 };
    cook.method = Some("cook/raw_shrimp".to_string());
    cook.progress_current = 5;
    let mut life = life_of(vec![cook], Some("n1"));
    record_block(&mut life, 0, &["raw_shrimp".to_string()]);
    let batch = life.nodes.iter().find_map(|node| match &node.kind {
        NodeKind::ObtainItems { item, count } if item == "raw_shrimp" => Some(*count),
        _ => None,
    });
    // Audit 2026-11-13 (A9): the rest of the tranche (live level 1 → 30
    // at 30 XP a shrimp), never one inventory load.
    assert_eq!(batch, crate::sizing::tranche_need("cook/raw_shrimp", 1, 30, "raw_shrimp"), "the block sized its batch from the cook method");
    assert_eq!(life.nodes[0].blocks, 1);
    assert_eq!(life.nodes[0].method, None, "the method clears once the sub-task is filed");
    super::record_progress(&mut life, 0, 5);
    assert_eq!(life.nodes[0].blocks, 1, "no level gained: the block still counts");
    super::record_progress(&mut life, 0, 9);
    assert_eq!(life.nodes[0].blocks, 0, "levels gained: the load did its job");
    assert_eq!(life.nodes[0].progress_current, 9);
}
