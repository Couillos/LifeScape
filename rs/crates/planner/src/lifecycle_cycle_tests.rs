//! Tests of `lifecycle_cycle.rs` on fabricated graphs (live 2026-11-17,
//! jim_ellis: `have_coins` hunting men blocked on herring, the herring
//! obtain blocked on coins, a fresh pair every pass).

use crate::lifecycle::{mark_launched, normalize, record_block, FailureFold};
use crate::model::{Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

fn life_of(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "jim_ellis".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["firemaking".to_string()], level: 90 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

fn node(id: &str, kind: NodeKind, deps: &[&str]) -> Node {
    Node::new(id, kind, deps.iter().map(|dep| dep.to_string()).collect(), 1)
}

fn position(life: &Life, id: &str) -> usize {
    life.nodes.iter().position(|node| node.id == id).unwrap_or_else(|| panic!("{id} in the graph"))
}

/// The funded axe step: obtain mithril_axe waits on a `have_coins` that
/// hunts men.
fn axe_step() -> Life {
    let axe = node("n474", NodeKind::ObtainItems { item: "mithril_axe".to_string(), count: 1 }, &["n476"]);
    let equip = node("n475", NodeKind::EquipBest { style: "melee".to_string() }, &["n474"]);
    let mut coins = node("n476", NodeKind::HaveCoins { amount: 339 }, &[]);
    coins.status = NodeStatus::Ready;
    coins.method = Some("fight:melee/men".to_string());
    let mut life = life_of(vec![axe, equip, coins]);
    mark_launched(&mut life, 2);
    life
}

/// Through the coins: the hunt blocks on herring, the herring obtain
/// blocks on coins the hunt was to earn. The pair is never filed again:
/// the herring obtain leaves, the coin step drops the hunt and resolves
/// another money method.
#[test]
fn a_coin_step_whose_food_needs_its_coins_drops_its_method() {
    let mut life = axe_step();
    assert_eq!(record_block(&mut life, 2, &["herring".to_string()]), FailureFold::Retry);
    assert_eq!(life.nodes[2].blocked_by.as_deref(), Some("fight:melee/men"));
    let herring = life.nodes.iter().position(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "herring")).expect("herring obtain filed");
    let herring_id = life.nodes[herring].id.clone();
    let before = life.nodes.len();
    // Blocked at resolve: no method of its own.
    let fold = record_block(&mut life, herring, &["coins:240".to_string()]);
    let FailureFold::CycleBroken(line) = fold else { panic!("cycle expected, got {fold:?}") };
    assert!(line.contains("which n476 works toward") && line.contains("broken at n476 (fight:melee/men)"), "{line}");
    assert!(line.contains("fight:melee/men excluded"), "{line}");
    assert_eq!(life.nodes.len(), before - 1, "no fresh have_coins, the herring obtain left");
    assert!(!life.nodes.iter().any(|node| node.id == herring_id));
    let coins = position(&life, "n476");
    assert_eq!(life.nodes[coins].excluded, vec!["fight:melee/men".to_string()]);
    assert!(life.nodes[coins].deps.is_empty());
    normalize(&mut life);
    assert_eq!(life.nodes[position(&life, "n476")].status, NodeStatus::Ready, "resolves another money method");
    assert_eq!(life.nodes[position(&life, "n474")].status, NodeStatus::Pending, "the axe still waits on the coins");
}

/// The live graph before the fix: hundreds of pairs unrolled, no
/// blocking method recorded. The outermost coin step keeps the axe, the
/// whole chain under it leaves.
#[test]
fn an_unrolled_legacy_chain_collapses_to_its_outermost_coin_step() {
    let mut life = axe_step();
    life.active = None;
    let mut nodes = vec![];
    for number in 477..=491 {
        let kind = if number % 2 == 1 { NodeKind::ObtainItems { item: "herring".to_string(), count: 20 } } else { NodeKind::HaveCoins { amount: 240 } };
        let next = format!("n{}", number + 1);
        let deps: Vec<&str> = if number < 491 { vec![next.as_str()] } else { Vec::new() };
        let mut fresh = node(&format!("n{number}"), kind, &deps);
        fresh.blocks = if number == 491 { 0 } else { 1 };
        fresh.status = if number == 491 { NodeStatus::Ready } else { NodeStatus::Pending };
        nodes.push(fresh);
    }
    let coins = position(&life, "n476");
    life.nodes[coins].status = NodeStatus::Pending;
    life.nodes[coins].method = None;
    life.nodes[coins].blocks = 1;
    life.nodes[coins].deps = vec!["n477".to_string()];
    life.nodes.extend(nodes);
    let last = position(&life, "n491");
    let fold = record_block(&mut life, last, &["coins:240".to_string()]);
    let FailureFold::CycleBroken(line) = fold else { panic!("cycle expected, got {fold:?}") };
    assert!(line.contains("15 node(s) dropped (n477..n491)"), "{line}");
    assert!(line.contains("blocking method unknown"), "{line}");
    assert_eq!(life.nodes.len(), 3, "{:?}", life.nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>());
    assert!(life.nodes[position(&life, "n476")].deps.is_empty());
    assert!(normalize(&mut life).iter().all(|change| !change.contains("abandon")));
}

/// Directly: A (smelting) blocks on ore; the ore obtain's own method
/// blocks on A's bar. Broken at A, the outermost: its smelting excluded,
/// the ore obtain leaves, no fresh bar obtain is filed.
#[test]
fn a_direct_pair_breaks_at_its_outer_node() {
    let mut bar = node("n1", NodeKind::ObtainItems { item: "iron_bar".to_string(), count: 10 }, &[]);
    bar.status = NodeStatus::Ready;
    bar.method = Some("smelt:iron_bar".to_string());
    let mut life = life_of(vec![bar]);
    mark_launched(&mut life, 0);
    assert_eq!(record_block(&mut life, 0, &["iron_ore".to_string()]), FailureFold::Retry);
    let ore = life.nodes.iter().position(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "iron_ore")).expect("ore obtain filed");
    life.nodes[ore].status = NodeStatus::Ready;
    life.nodes[ore].method = Some("farm:iron_ore".to_string());
    mark_launched(&mut life, ore);
    let fold = record_block(&mut life, ore, &["iron_bar".to_string()]);
    let FailureFold::CycleBroken(line) = fold else { panic!("cycle expected, got {fold:?}") };
    assert!(line.contains("smelt:iron_bar excluded"), "{line}");
    assert_eq!(life.nodes.len(), 1, "the ore obtain left, nothing filed");
    assert_eq!(life.nodes[0].excluded, vec!["smelt:iron_bar".to_string()]);
    assert!(life.nodes[0].deps.is_empty());
    assert!(life.active.is_none());
}

/// The live jim_ellis graph at 11:00: the innermost coin step hunts men
/// (its method known), every outer node unrolled before the blocking
/// method was recorded. The outermost coin step, a repeat of the blocked
/// one, drops that hunt; everything under it leaves.
#[test]
fn an_unrolled_chain_breaks_at_its_outermost_repeat_with_the_inner_method() {
    let mut life = axe_step();
    life.active = None;
    let coins = position(&life, "n476");
    life.nodes[coins].status = NodeStatus::Pending;
    life.nodes[coins].method = None;
    life.nodes[coins].blocks = 1;
    life.nodes[coins].deps = vec!["n477".to_string()];
    let mut herring = node("n477", NodeKind::ObtainItems { item: "herring".to_string(), count: 20 }, &["n478"]);
    herring.blocks = 1;
    let mut climb = node("n478", NodeKind::ReachLevel { skill: "cooking".to_string(), level: 5 }, &["n479"]);
    climb.blocks = 0;
    let mut shrimp = node("n479", NodeKind::ObtainItems { item: "raw_shrimp".to_string(), count: 13 }, &["n480"]);
    shrimp.blocks = 1;
    let mut inner = node("n480", NodeKind::HaveCoins { amount: 1014 }, &[]);
    inner.status = NodeStatus::Ready;
    inner.method = Some("fight:melee/men".to_string());
    life.nodes.extend([herring, climb, shrimp, inner]);
    let at = position(&life, "n480");
    mark_launched(&mut life, at);
    let fold = record_block(&mut life, at, &["herring".to_string()]);
    let FailureFold::CycleBroken(line) = fold else { panic!("cycle expected, got {fold:?}") };
    assert!(line.contains("n480 needs herring, which n477 works toward") && line.contains("broken at n476 (fight:melee/men)"), "{line}");
    assert!(line.contains("4 node(s) dropped (n477..n480)"), "{line}");
    assert_eq!(life.nodes.len(), 3);
    assert_eq!(life.nodes[position(&life, "n476")].excluded, vec!["fight:melee/men".to_string()]);
    assert!(life.active.is_none());
}

/// No cycle: a node waiting on the blocked one by plan (never blocked
/// itself) that obtains the same object is a later need, a fresh obtain
/// serves (owen_wood n11/n16).
#[test]
fn a_planned_successor_is_no_twin() {
    let mut climb = node("n11", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 5 }, &[]);
    climb.status = NodeStatus::Ready;
    climb.method = Some("smith:bronze_dagger".to_string());
    let bars = node("n16", NodeKind::ObtainItems { item: "bronze_bar".to_string(), count: 48 }, &["n11"]);
    let mut life = life_of(vec![climb, bars]);
    assert_eq!(record_block(&mut life, 0, &["bronze_bar".to_string()]), FailureFold::Retry);
    assert!(life.nodes[0].excluded.is_empty());
}



/// A graph unrolled before the breaker existed: the innermost coin step
/// is about to resolve under a blocked coin step. Earning coins never
/// waits on earning coins: the outermost one takes over, the chain
/// leaves.
#[test]
fn a_coin_step_under_a_blocked_coin_step_collapses_before_it_resolves() {
    let mut life = axe_step();
    life.active = None;
    let coins = position(&life, "n476");
    life.nodes[coins].status = NodeStatus::Pending;
    life.nodes[coins].method = None;
    life.nodes[coins].blocks = 1;
    life.nodes[coins].deps = vec!["n477".to_string()];
    let mut herring = node("n477", NodeKind::ObtainItems { item: "herring".to_string(), count: 20 }, &["n478"]);
    herring.blocks = 1;
    let mut inner = node("n478", NodeKind::HaveCoins { amount: 240 }, &[]);
    inner.status = NodeStatus::Ready;
    life.nodes.extend([herring, inner]);
    let at = position(&life, "n478");
    let found = crate::lifecycle_cycle::coin_regress(&life, at);
    assert_eq!(found, vec![position(&life, "n476")]);
    let fold = crate::lifecycle_cycle::break_cycle(&mut life, at, "coins", &found);
    let FailureFold::CycleBroken(line) = fold else { panic!("cycle expected, got {fold:?}") };
    assert!(line.contains("2 node(s) dropped (n477..n478)"), "{line}");
    assert_eq!(life.nodes.len(), 3);
    // A plain coin step (no coin node waits on it blocked) is no cycle.
    let fresh = axe_step();
    assert!(crate::lifecycle_cycle::coin_regress(&fresh, 2).is_empty());
}
