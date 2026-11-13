//! Tests of `quests.rs`: quest prerequisites are dependencies, never a
//! launch through a missing quest (user decision 2026-11-13).

use super::{method_quest, missing_quest, quest_point_ladder, quests_done_from, quests_done_in_graph};
use crate::expand::{resolve_ready, ReadyCtx};
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::Produce { item: "blankrune".to_string(), count: 100 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn pickaxe_ctx() -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.tools = vec!["bronze_pickaxe".to_string()];
    ctx
}

/// Server content: essence sits behind Rune Mysteries
/// (`essence_mine.rs2`), Oziach sells rune plate after Dragon Slayer; a
/// plain ore or a shears shop needs nothing.
#[test]
fn methods_name_their_server_quest() {
    assert_eq!(method_quest("essence:blankrune", None).as_deref(), Some("runemysteries"));
    assert_eq!(method_quest("farm:blankrune", None).as_deref(), Some("runemysteries"));
    assert_eq!(method_quest("farm:copper_ore", None), None);
    assert_eq!(method_quest("shop:buy", Some("rune_platebody")).as_deref(), Some("dragon"));
    assert_eq!(method_quest("shop:buy", Some("shears")), None);
    let mut ctx = pickaxe_ctx();
    assert!(!crate::tools::method_feasible("essence:blankrune", &ctx), "no essence before the quest");
    ctx.quests_done = vec!["runemysteries".to_string()];
    assert_eq!(missing_quest("essence:blankrune", None, &ctx), None);
    assert!(crate::tools::method_feasible("essence:blankrune", &ctx));
}

/// A production quota of essence without Rune Mysteries blocks on the
/// quest (never launches the essence run); with it, the leg locks.
#[test]
fn essence_quota_blocks_on_rune_mysteries_then_mines() {
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 100 }, Vec::new(), 100);
    node.alternatives = crate::obtain_legs::produce_alternatives("blankrune");
    assert!(!node.alternatives.is_empty(), "essence has a production leg");
    let mut ctx = pickaxe_ctx();
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:quest:runemysteries".to_string() }, "method {:?}", node.method);
    assert_eq!(node.method, None);
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 100 }, Vec::new(), 100);
    node.alternatives = crate::obtain_legs::produce_alternatives("blankrune");
    ctx.quests_done = vec!["runemysteries".to_string()];
    resolve_ready(&mut node, &ctx);
    assert!(node.method.is_some(), "quest done: the essence leg runs, got {:?}", node.status);
}

/// The quest block becomes the quest's own nodes (one open quest node,
/// reused on the next block), the blocked node waiting on it.
#[test]
fn a_quest_block_plans_the_quest_as_a_dependency() {
    let node = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 100 }, Vec::new(), 100);
    let mut life = life_of(vec![node]);
    crate::lifecycle::record_block(&mut life, 0, &["quest:runemysteries".to_string()]);
    let quest = life
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "runemysteries"))
        .expect("the quest is planned")
        .id
        .clone();
    assert!(life.nodes[0].deps.contains(&quest), "the essence waits on the quest");
    let quests = life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::CompleteQuest { .. })).count();
    crate::lifecycle::record_block(&mut life, 0, &["quest:runemysteries".to_string()]);
    let again = life.nodes.iter().filter(|node| matches!(node.kind, NodeKind::CompleteQuest { .. })).count();
    assert_eq!(quests, again, "an open quest node is reused");
    let (script, _) = crate::mapping::map_method("quest:runemysteries", &life.nodes[0], None).expect("quest maps");
    assert_eq!(script, "quest_rune_mysteries");
}

/// An area block (`access:<area>`) files the area's own gates: the
/// Crafting Guild climbs crafting to 40 and obtains the brown apron.
#[test]
fn an_access_block_plans_the_area_gates() {
    let node = Node::new("n1", NodeKind::ObtainItems { item: "gold_ore".to_string(), count: 10 }, Vec::new(), 10);
    let mut life = life_of(vec![node]);
    crate::lifecycle::record_block_from(&mut life, 0, &["access:crafting_guild".to_string()], &|_| 30);
    let access = life.nodes.iter().find(|node| matches!(&node.kind, NodeKind::AccessArea { area } if area == "crafting_guild")).expect("access planned");
    assert!(life.nodes[0].deps.contains(&access.id));
    assert!(life.nodes.iter().any(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "brown_apron")));
    assert!(life.nodes.iter().any(|node| matches!(&node.kind, NodeKind::ReachLevel { skill, level: 40 } if skill == "crafting")));
    let ids: std::collections::HashSet<&str> = life.nodes.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(ids.len(), life.nodes.len(), "fresh ids never collide");
}

/// Completion from the engine save (varp 63 `runemysteries` at 6) or a
/// done quest node of the life.
#[test]
fn completion_reads_varps_and_the_graph() {
    let varp = lifescape_gamedata::varps::id_by_name("runemysteries").expect("varp");
    let done = quests_done_from(&|id| if id == varp { 6 } else { 0 });
    assert_eq!(done, vec!["runemysteries".to_string()]);
    assert!(quests_done_from(&|id| if id == varp { 5 } else { 0 }).is_empty(), "notes received is not complete");
    let mut quest = Node::new("n2", NodeKind::CompleteQuest { quest: "prince".to_string() }, Vec::new(), 1);
    quest.status = NodeStatus::Done;
    assert_eq!(quests_done_in_graph(&life_of(vec![quest])), vec!["prince".to_string()]);
}

/// The quest-point ladder (replaces the Cook's Assistant collapse): Dragon
/// Slayer's 32 points come from real rungs, cheapest points first, a
/// points-gated rung only after its gate, never a quest no bot runs solo.
#[test]
fn quest_points_climb_a_real_ladder() {
    let points = |keys: &[String]| -> i64 {
        keys.iter().map(|key| lifescape_gamedata::quest_requirements::by_key(key).map(|quest| quest.reward_points).unwrap_or(0)).sum()
    };
    let ladder = quest_point_ladder(32, "dragon", &[]);
    assert!(points(&ladder) >= 32, "{ladder:?}");
    assert!(!ladder.iter().any(|key| key == "dragon" || key == "blackarmgang" || key == "hunt"), "{ladder:?}");
    if let Some(at) = ladder.iter().position(|key| key == "blackknight") {
        assert!(points(&ladder[..at].to_vec()) >= 12, "Black Knights' Fortress after 12 points: {ladder:?}");
    }
    // Done quests count: a bot holding Romeo and Goblin Diplomacy (10)
    // needs two more for the fortress gate.
    let done = vec!["romeojuliet".to_string(), "gobdip".to_string()];
    let rest = quest_point_ladder(12, "blackknight", &done);
    assert!(points(&rest) >= 2 && !rest.contains(&"romeojuliet".to_string()), "{rest:?}");
    assert!(quest_point_ladder(5, "dragon", &["romeojuliet".to_string()]).is_empty(), "already held");
    // The expansion files the rungs before the quest node.
    let nodes = crate::expand::expand_goal(&GoalKind::CompleteQuest { quest: "dragon".to_string() }, &Profile::new(false, 0.5, 0.5, 7, Archetype::Producer));
    let quests: Vec<String> = nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::CompleteQuest { quest } => Some(quest.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(quests.last().map(String::as_str), Some("dragon"));
    assert!(points(&quests) - 2 >= 32, "rungs worth 32 points: {quests:?}");
    assert!(!quests[..quests.len() - 1].contains(&"dragon".to_string()));
}
