//! Tests of the named status moves of `lifecycle.rs` used by the brain
//! (report fold, launch pass, market wait, tutorial gate): no status is
//! written outside the lifecycle.

use super::{complete_node, end_engagement, insert_ready, mark_launched, normalize, reached_live, record_block_from, retry_uncounted};
use crate::model::{Archetype, Engagement, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["attack".to_string()], level: 55 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
    }
}

fn running(kind: NodeKind) -> Life {
    let mut node = Node::new("n1", kind, Vec::new(), 10);
    node.status = NodeStatus::Ready;
    node.method = Some("fight:melee/cows".to_string());
    let mut life = life_of(vec![node]);
    mark_launched(&mut life, 0);
    life
}

#[test]
fn complete_node_closes_releases_and_ends_its_engagement() {
    let mut life = running(NodeKind::MakeMoney);
    life.engagement = Some(Engagement { node: "n1".to_string(), min_ticks: 6000, max_ticks: 9000, chosen: 9000 });
    complete_node(&mut life, 0, 42);
    assert_eq!(life.nodes[0].status, NodeStatus::Done);
    assert_eq!(life.nodes[0].progress_current, 42, "the caller's proof is the progress");
    assert!(life.active.is_none(), "lock released");
    assert!(life.engagement.is_none(), "the engagement of a done node ends");
    assert!(normalize(&mut life).is_empty(), "already normal");
}

#[test]
fn retry_uncounted_never_spends_the_failure_budget() {
    let mut life = running(NodeKind::ReachLevel { skill: "attack".to_string(), level: 10 });
    retry_uncounted(&mut life, 0);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert!(life.nodes[0].method.is_none(), "fresh resolve next pass");
    assert!(life.active.is_none());
    assert_eq!((life.nodes[0].failures, life.nodes[0].blocks), (0, 0), "uncounted");
}

#[test]
fn end_engagement_closes_money_and_retries_levels() {
    let mut money = running(NodeKind::MakeMoney);
    end_engagement(&mut money, 0);
    assert_eq!(money.nodes[0].status, NodeStatus::Done, "one money engagement is the task");
    assert!(money.active.is_none());
    let mut level = running(NodeKind::ReachLevel { skill: "attack".to_string(), level: 10 });
    end_engagement(&mut level, 0);
    assert_eq!(level.nodes[0].status, NodeStatus::Ready, "a level keeps climbing");
    assert!(level.active.is_none());
}

#[test]
fn insert_ready_files_a_launchable_node() {
    let mut life = life_of(Vec::new());
    let index = insert_ready(&mut life, Node::new("n1", NodeKind::SpaceCleanup, Vec::new(), 0));
    assert_eq!(index, 0);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready);
    assert!(normalize(&mut life).is_empty(), "ready without deps is normal");
}

/// A level a production needs climbs from the live level: no tranche at
/// or below what the bot already has (smithing 16 needs only 16 -> 18).
#[test]
fn level_blocks_climb_from_the_live_level() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "steel_bar".to_string(), count: 1 }, Vec::new(), 1);
    node.status = NodeStatus::Ready;
    let mut life = life_of(vec![node]);
    mark_launched(&mut life, 0);
    let live = |skill: &str| if skill == "smithing" { 16 } else { 1 };
    record_block_from(&mut life, 0, &["level:smithing:18".to_string()], &live);
    let climbs: Vec<(u8, String)> = life
        .nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::ReachLevel { skill, level } if skill == "smithing" => {
                Some((*level, node.alternatives.first().map(|alt| alt.method.clone()).unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(climbs.len(), 1, "one tranche 16 -> 18, got {climbs:?}");
    assert_eq!(climbs[0].0, 18);
    assert_eq!(climbs[0].1, crate::tranches::reach_method("smithing", 16), "the tier open at the live level");
}

/// b8jxrdd2: "attack 10" ran 18 015 ticks at attack 45. The live level
/// proves a stale tranche done; unknown levels (1) never do.
#[test]
fn live_levels_prove_stale_climbs_done() {
    let attack = Node::new("n1", NodeKind::ReachLevel { skill: "attack".to_string(), level: 10 }, Vec::new(), 10);
    assert_eq!(reached_live(&attack, &|skill| if skill == "attack" { 45 } else { 1 }, 3), Some(45));
    assert_eq!(reached_live(&attack, &|_| 1, 3), None, "unknown level never closes");
    assert_eq!(reached_live(&attack, &|_| 9, 3), None, "one short still climbs");
    let combat = Node::new("n2", NodeKind::ReachCombat { level: 15 }, Vec::new(), 15);
    assert_eq!(reached_live(&combat, &|_| 1, 20), Some(20));
    assert_eq!(reached_live(&combat, &|_| 99, 14), None, "combat reads the combat level");
    let obtain = Node::new("n3", NodeKind::ObtainItems { item: "feather".to_string(), count: 1 }, Vec::new(), 1);
    assert_eq!(reached_live(&obtain, &|_| 99, 126), None, "only climbs");
}
