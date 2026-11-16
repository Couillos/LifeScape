//! Tests of `quest_hold.rs` (live 2026-11-14, bh99tw2f: every banked air
//! talisman locked by its own listings, Rune Mysteries looped).

use super::{cooled_quest_nodes, cooled_quests, open_quests, quest_ban_key, quest_shortfalls, reserved_quest_objects, withhold_reserved, QuestShortfall};
use crate::model::{Archetype, GoalKind, HistoryEntry, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>, history: Vec<HistoryEntry>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::Produce { item: "blankrune".to_string(), count: 200 },
        nodes,
        engagement: None,
        active: None,
        history,
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn quest(id: &str, key: &str, status: NodeStatus) -> Node {
    let mut node = Node::new(id, NodeKind::CompleteQuest { quest: key.to_string() }, Vec::new(), 1);
    node.status = status;
    node
}

/// An open quest reserves its objects (handed and brought, summed across
/// quests); a done or abandoned one and the tutorial reserve nothing.
#[test]
fn open_quests_reserve_their_objects() {
    let life = life_of(
        vec![
            quest("n1", "runemysteries", NodeStatus::Ready),
            quest("n2", "sheep", NodeStatus::Pending),
            quest("n3", "cook", NodeStatus::Done),
            quest("n4", "tutorial", NodeStatus::Ready),
            quest("n5", "runemysteries", NodeStatus::Pending),
        ],
        Vec::new(),
    );
    assert_eq!(open_quests(&life), vec!["runemysteries".to_string(), "sheep".to_string()], "unique, done and tutorial out");
    let reserved = reserved_quest_objects(&life);
    assert!(reserved.contains(&("air_talisman".to_string(), 1)), "{reserved:?}");
    assert!(reserved.contains(&("ball_of_wool".to_string(), 20)));
    assert!(!reserved.iter().any(|(name, _)| name == "bucket_milk"), "a done quest holds nothing");
}

/// bh99tw2f: 10 talismans, the quest keeps one, the sale offers 9.
#[test]
fn sales_offer_what_the_quests_do_not_keep() {
    let stacks = vec![("air_talisman".to_string(), 10), ("ball_of_wool".to_string(), 5), ("raw_trout".to_string(), 134)];
    let reserved = vec![("air_talisman".to_string(), 1), ("ball_of_wool".to_string(), 20)];
    assert_eq!(withhold_reserved(&stacks, &reserved), vec![("air_talisman".to_string(), 9), ("raw_trout".to_string(), 134)]);
}

/// The launch gate: a talisman held but locked is released first; held
/// nowhere the Duke hands it; a brought item short is obtained first.
#[test]
fn quests_launch_with_their_objects_at_hand() {
    let locked = |name: &str| if name == "air_talisman" { (10, 0) } else { (0, 0) };
    assert_eq!(
        quest_shortfalls("runemysteries", &locked),
        vec![QuestShortfall::Locked { item: "air_talisman".to_string(), need: 1, held: 10, free: 0 }]
    );
    assert!(quest_shortfalls("runemysteries", &locked)[0].describe().contains("10 locked by open market sales"));
    assert!(quest_shortfalls("runemysteries", &|_| (0, 0)).is_empty(), "the Duke hands the talisman at stage 0");
    assert!(quest_shortfalls("runemysteries", &|_| (10, 9)).is_empty(), "one free unit is enough");
    let wool = |name: &str| if name == "ball_of_wool" { (5, 5) } else { (0, 0) };
    assert_eq!(quest_shortfalls("sheep", &wool), vec![QuestShortfall::Missing { item: "ball_of_wool".to_string(), need: 20, held: 5 }]);
    let listed_wool = |name: &str| if name == "ball_of_wool" { (25, 10) } else { (0, 0) };
    assert!(matches!(&quest_shortfalls("sheep", &listed_wool)[..], [QuestShortfall::Locked { need: 20, held: 25, free: 10, .. }]));
    assert!(quest_shortfalls("demon", &|_| (0, 0)).iter().all(|gap| !gap.describe().starts_with("coins")), "coins are the wallet's");
}

/// A failed quest cools as a quest: its ready nodes wait, other quests
/// and an expired cooldown launch.
#[test]
fn a_failed_quest_cools_as_a_quest() {
    let stuck = crate::bans::StuckCause { node: "n416".to_string(), class: "3 failures".to_string(), last_error: "no air talisman".to_string() };
    let history = vec![HistoryEntry { node: "goal".to_string(), started: 100, ended: 100, outcome: crate::bans::ban_entry(&quest_ban_key("runemysteries"), 6000, &stuck) }];
    let life = life_of(vec![quest("n1", "runemysteries", NodeStatus::Ready), quest("n2", "cook", NodeStatus::Ready)], history);
    let cooled = cooled_quests(&life.history, &|_| 10);
    assert_eq!(cooled, vec!["runemysteries".to_string()]);
    assert_eq!(cooled_quest_nodes(&life, &cooled), vec!["n1".to_string()]);
    assert!(cooled_quests(&life.history, &|_| 6001).is_empty(), "the cooldown expires");
}

/// A quest short of a brought item waits on an obtain of the quest's
/// count (20 balls of wool), like any other dependency.
#[test]
fn a_quest_short_of_an_item_obtains_its_count_first() {
    let mut life = life_of(vec![quest("n1", "sheep", NodeStatus::Ready)], Vec::new());
    crate::lifecycle::record_block(&mut life, 0, &["ball_of_wool".to_string()]);
    let obtain = life
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "ball_of_wool"))
        .expect("obtain filed");
    assert_eq!(obtain.kind, NodeKind::ObtainItems { item: "ball_of_wool".to_string(), count: 20 });
    assert!(life.nodes[0].deps.contains(&obtain.id), "the quest waits on it");
    assert_eq!(life.nodes[0].status, NodeStatus::Pending);
}
