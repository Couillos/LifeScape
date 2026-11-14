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
    assert!(!ladder.iter().any(|key| key == "dragon" || key == "blackarmgang"), "{ladder:?}");
    // Pirate's Treasure sails (the Karamja boat has nav edges): a rung.
    assert!(quest_point_ladder(1_000, "dragon", &[]).contains(&"hunt".to_string()), "the boat opens Pirate's Treasure");
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

/// Live 2026-11-13 (bxkchh6x, leo_king, nina_rhodes, sara_miller): the
/// quest script reported `goal_reached` without finishing Rune Mysteries,
/// the done quest node then closed every later quest node "already
/// complete" while the essence run kept blocking on the quest (4 blocks,
/// mining banned). A block on `quest:X` is live proof X is NOT done: it
/// voids every done quest node of X, and the fresh quest node launches.
#[test]
fn a_quest_block_disproves_done_quest_nodes() {
    let essence = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 39 }, Vec::new(), 39);
    let mut quest = Node::new("n2", NodeKind::CompleteQuest { quest: "runemysteries".to_string() }, Vec::new(), 1);
    quest.status = NodeStatus::Done;
    let mut prince = Node::new("n3", NodeKind::CompleteQuest { quest: "prince".to_string() }, Vec::new(), 1);
    prince.status = NodeStatus::Done;
    let mut life = life_of(vec![essence, quest, prince]);
    assert!(quests_done_in_graph(&life).contains(&"runemysteries".to_string()));
    crate::lifecycle::record_block(&mut life, 0, &["quest:runemysteries".to_string()]);
    assert_eq!(quests_done_in_graph(&life), vec!["prince".to_string()], "the block voids the false proof, other quests stand");
    let fresh = life
        .nodes
        .iter()
        .find(|node| node.id != "n2" && matches!(&node.kind, NodeKind::CompleteQuest { quest } if quest == "runemysteries"))
        .expect("a fresh quest node is filed");
    assert!(life.nodes[0].deps.contains(&fresh.id), "the essence waits on the fresh quest node");
    assert_eq!(life.nodes[1].status, NodeStatus::Done, "history stays: the status is never rewritten");
}

/// Live 2026-11-14 (bbkjz2vk n639, b3gybcev n150): without Rune
/// Mysteries the essence of a runecraft tranche went to a resting market
/// bid nobody could fill (no bot had the quest), failed after 6000 ticks
/// and banned runecraft and mining. A light quest that unlocks the way
/// now comes first; a covered ask still buys at once.
#[test]
fn essence_without_the_quest_waits_on_the_quest_not_a_resting_bid() {
    let mut ctx = pickaxe_ctx();
    ctx.coins = 5000;
    ctx.price = 4;
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 17 }, Vec::new(), 17);
    node.alternatives = crate::obtain_legs::obtain_alternatives("blankrune", false);
    assert!(node.alternatives.iter().any(|alt| alt.method == "market:buy"), "essence trades: {:?}", node.alternatives);
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:quest:runemysteries".to_string() }, "method {:?}", node.method);
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "blankrune".to_string(), count: 17 }, Vec::new(), 17);
    node.alternatives = crate::obtain_legs::obtain_alternatives("blankrune", false);
    ctx.best_ask = Some(5);
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.method.as_deref(), Some("market:buy"), "a covered ask buys: {:?}", node.status);
    assert!(super::light_quest("runemysteries"));
    assert!(!super::light_quest("dragon"), "Dragon Slayer is no light quest");
}

/// Talismans past air drop only (server drop tables): earth from men and
/// body from guards farm on combat rungs; mind (imps, wizards), water and
/// fire (wizards, dark wizards, sewer skeletons) have no rung, so the
/// runecraft tranches skip their tiers (`tranches::talisman_obtainable`).
#[test]
fn talisman_legs_follow_the_combat_rungs() {
    for talisman in ["earth_talisman", "body_talisman"] {
        let legs = crate::obtain_legs::production_legs(talisman);
        assert!(legs.iter().any(|leg| leg.method == format!("farm:{talisman}")), "{talisman}: {legs:?}");
    }
    for talisman in ["mind_talisman", "water_talisman", "fire_talisman"] {
        assert!(!crate::tranches::talisman_obtainable(talisman), "{talisman} has no rung: a new rung lifts this");
    }
    assert!(crate::tranches::talisman_obtainable("air_talisman"), "Rune Mysteries hands it over");
}

/// Rune Mysteries hands the air talisman over (`duke_horacio.rs2`, back
/// from Sedridor at completion): the obtain waits on the quest instead of
/// farming goblins (0.78 % a kill) or bidding; once the quest is done the
/// usual legs come back.
#[test]
fn the_air_talisman_comes_from_rune_mysteries() {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 5000;
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "air_talisman".to_string(), count: 1 }, Vec::new(), 1);
    node.alternatives = crate::obtain_legs::obtain_alternatives("air_talisman", false);
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:quest:runemysteries".to_string() }, "method {:?}", node.method);
    assert_eq!(node.method, None);
    ctx.quests_done = vec!["runemysteries".to_string()];
    let mut node = Node::new("n1", NodeKind::ObtainItems { item: "air_talisman".to_string(), count: 1 }, Vec::new(), 1);
    node.alternatives = crate::obtain_legs::obtain_alternatives("air_talisman", false);
    resolve_ready(&mut node, &ctx);
    let quest_block = matches!(&node.status, NodeStatus::Blocked { reason } if reason.contains("quest"));
    assert!(!quest_block, "quest done: a lost talisman takes the usual legs, got {:?} {:?}", node.method, node.status);
    assert_eq!(super::granting_quest("mind_talisman", &ReadyCtx::blind(0.5)), None, "only the air talisman is a quest grant");
}
