//! Live 2026-11-14, ryan_mason (mage pinned magic 99, 0 coins): 30+ min
//! idle, one fresh node every ~1.5 min, alternating `obtain airrune 1
//! blocked at resolve (missing:combat:43)` and `reach_combat 43 blocked
//! (missing airrune)`. Two faults:
//! - an obtain the market sells never asked for the coins of the
//!   purchase: its field legs (ground spawn, drop) only had sites safe
//!   at combat 43, so the block named the combat level — 39 levels of
//!   training for one 1-coin rune, and no money method ever ran;
//! - the block filed a fresh node equal to one that already waited on
//!   the blocked node (a dependency cycle), unrolled forever.

use crate::expand::{resolve_ready, ReadyCtx};
use crate::lifecycle::{normalize, record_block, FailureFold};
use crate::model::{Alternative, Archetype, GoalKind, Life, Node, NodeKind, NodeStatus, Profile};

const LUMBRIDGE: (i64, i64, i64) = (3232, 3293, 0);

fn rune_obtain(id: &str, count: i64) -> Node {
    let mut node = Node::new(id, NodeKind::ObtainItems { item: "airrune".to_string(), count }, Vec::new(), count);
    for method in ["market:buy", "ground:airrune", "farm:airrune", "runecraft:airrune"] {
        node.alternatives.push(Alternative { method: method.to_string(), est_ticks: 0, est_coins: 0 });
    }
    node.status = NodeStatus::Ready;
    node
}

fn broke_fresh_mage() -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins = 0;
    ctx.combat = 4;
    ctx.position = Some(LUMBRIDGE);
    ctx.price = 1;
    ctx.best_ask = Some(1);
    ctx.levels = ["attack", "strength", "defence", "magic", "runecraft"].iter().map(|skill| (skill.to_string(), 1)).collect();
    ctx.levels.push(("hitpoints".to_string(), 10));
    ctx
}

/// A broke fresh bot needing one air rune the market sells: the block
/// is the coins of the purchase (a `have_coins` node earns them), never
/// the combat level of the drop or ground sites.
#[test]
fn a_broke_bot_earns_the_coins_of_a_market_input_instead_of_training_combat() {
    let ctx = broke_fresh_mage();
    let mut node = rune_obtain("n10", 1);
    resolve_ready(&mut node, &ctx);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:coins:1".to_string() }, "{:?} {:?} {:?}", node.method, node.site, node.alternatives);
    // 173 runes at the 1-coin ask: the wallet of the whole purchase.
    let mut bulk = rune_obtain("n2", 173);
    resolve_ready(&mut bulk, &ctx);
    assert_eq!(bulk.status, NodeStatus::Blocked { reason: "missing:coins:173".to_string() }, "{:?}", bulk.alternatives);
}

/// Without a market leg (an Iron bot never trades) the combat level of
/// the field sites stays the honest block.
#[test]
fn without_a_market_leg_the_combat_level_stays_the_block() {
    let mut ctx = broke_fresh_mage();
    ctx.ironman = true;
    let mut node = rune_obtain("n10", 1);
    resolve_ready(&mut node, &ctx);
    match &node.status {
        NodeStatus::Blocked { reason } => assert!(!reason.starts_with("missing:coins"), "{reason}"),
        other => panic!("blocked expected, got {other:?}"),
    }
}

fn mage_life(nodes: Vec<Node>) -> Life {
    Life {
        schema: 1,
        account: "ryan_mason".to_string(),
        created_at: 0,
        seed: 7,
        profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
        goal: GoalKind::MaxSkills { skills: vec!["magic".to_string()], level: 10 },
        nodes,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
        memory: Default::default(),
    }
}

/// The ryan_mason chain: a combat climb blocked on air runes files an
/// obtain; that obtain blocked on the same combat level is a cycle,
/// never a fresh `reach_combat` that repeats the pair forever. Broken at
/// once by an alternative (user 2026-11-17): the obtain leaves the graph,
/// the climb drops the method that needed the runes and resolves again.
#[test]
fn a_block_that_needs_what_waits_on_it_is_a_cycle_never_unrolled() {
    let mut climb = Node::new("n6", NodeKind::ReachCombat { level: 43 }, Vec::new(), 43);
    climb.alternatives.push(Alternative { method: "fight:magic/chickens".to_string(), est_ticks: 0, est_coins: 0 });
    climb.status = NodeStatus::Ready;
    climb.method = Some("fight:magic/chickens".to_string());
    let mut life = mage_life(vec![climb]);
    assert_eq!(record_block(&mut life, 0, &["airrune".to_string()]), FailureFold::Retry);
    let obtain_at = life.nodes.iter().position(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "airrune")).expect("obtain filed");
    let before = life.nodes.len();
    let fold = record_block(&mut life, obtain_at, &["combat:43".to_string()]);
    let FailureFold::CycleBroken(line) = fold else { panic!("a cycle is broken, got {fold:?}") };
    assert!(line.contains("broken at n6 (fight:magic/chickens)"), "{line}");
    assert_eq!(life.nodes.len(), before - 1, "the rune obtain left, no fresh reach_combat filed");
    assert!(life.nodes[0].deps.is_empty(), "{:?}", life.nodes[0].deps);
    assert_eq!(life.nodes[0].excluded, vec!["fight:magic/chickens".to_string()]);
    normalize(&mut life);
    assert_eq!(life.nodes[0].status, NodeStatus::Ready, "the climb resolves another method");
}

/// Not a cycle: a smithing climb blocked on bars while the next
/// tranche's bar obtain waits on it (owen_wood n11/n16) gets a fresh
/// obtain of its own, as before.
#[test]
fn a_climb_blocked_on_the_next_tranches_input_is_no_cycle() {
    let mut climb = Node::new("n11", NodeKind::ReachLevel { skill: "smithing".to_string(), level: 5 }, Vec::new(), 5);
    climb.status = NodeStatus::Ready;
    let mut bars = Node::new("n16", NodeKind::ObtainItems { item: "bronze_bar".to_string(), count: 48 }, vec!["n11".to_string()], 48);
    bars.status = NodeStatus::Pending;
    let mut life = mage_life(vec![climb, bars]);
    assert_eq!(record_block(&mut life, 0, &["bronze_bar".to_string()]), FailureFold::Retry);
    assert_eq!(life.nodes[0].status, NodeStatus::Pending, "waits on a fresh obtain");
    assert!(life.nodes.len() > 2, "a fresh bar obtain filed");
}
