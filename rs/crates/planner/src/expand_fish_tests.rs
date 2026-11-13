//! A fish quota expands its real gear (user decision 2026-11-13):
//! b8jxrdd2 (fishing 50, goal raw_trout) was never asked for feathers
//! and bait-fished pike. Trout: fly rod + one feather per catch; pike:
//! rod + one fishing bait per catch; feathers and bait come from the
//! market or a farm (chickens, men), never an NPC shop.

use super::{expand_goal, resolve_ready, ReadyCtx};
use crate::model::{Archetype, GoalKind, Life, NodeKind, NodeStatus, Profile};

fn quota(item: &str, count: i64) -> Life {
    let profile = Profile::new(false, 0.5, 0.5, 7, Archetype::Producer);
    let goal = GoalKind::Produce { item: item.to_string(), count };
    Life {
        schema: 1,
        account: "t00".to_string(),
        created_at: 0,
        seed: 7,
        profile: profile.clone(),
        nodes: expand_goal(&goal, &profile),
        goal,
        engagement: None,
        active: None,
        history: Vec::new(),
        ambition: None,
        goal_origin: None,
    }
}

fn fisher(tools: &[&str]) -> ReadyCtx {
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.levels = vec![("fishing".to_string(), 50)];
    ctx.coins = 1000;
    ctx.tools = tools.iter().map(|tool| tool.to_string()).collect();
    ctx
}

#[test]
fn a_trout_quota_needs_the_fly_rod_and_its_feathers() {
    let mut life = quota("raw_trout", 100);
    let methods: Vec<&str> = life.nodes[0].alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert_eq!(methods, vec!["farm:raw_trout"], "a quota is produced, never bought");
    // No rod: the rod is the first need, broke or rich (no executor buys
    // its own gear: the obtain leg buys it at a shop before the launch).
    let mut node = life.nodes[0].clone();
    let mut broke = fisher(&[]);
    broke.coins = 0;
    resolve_ready(&mut node, &broke);
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:fly_fishing_rod".to_string() });
    let mut node = life.nodes[0].clone();
    resolve_ready(&mut node, &fisher(&[]));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:fly_fishing_rod".to_string() }, "a fundable rod is no held rod");
    // Rod carried: the feathers block (1000 coins never buy them at a shop).
    let mut node = life.nodes[0].clone();
    resolve_ready(&mut node, &fisher(&["fly_fishing_rod"]));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:feather".to_string() });
    // The block files one feather per trout, from the market or chickens.
    crate::lifecycle::record_block(&mut life, 0, &["feather".to_string()]);
    let feathers = life
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, NodeKind::ObtainItems { item, .. } if item == "feather"))
        .expect("feathers are planned");
    assert!(matches!(feathers.kind, NodeKind::ObtainItems { count: 100, .. }), "one feather per catch: {:?}", feathers.kind);
    let legs: Vec<&str> = feathers.alternatives.iter().map(|alt| alt.method.as_str()).collect();
    assert!(legs.contains(&"market:buy") && legs.contains(&"farm:feather"), "market or chickens: {legs:?}");
    assert!(!legs.contains(&"shop:buy"), "never a shop: {legs:?}");
    // Rod and feathers held: the leg runs and names the trout.
    let mut node = quota("raw_trout", 100).nodes[0].clone();
    resolve_ready(&mut node, &fisher(&["fly_fishing_rod", "feather"]));
    assert_eq!(node.method.as_deref(), Some("farm:raw_trout"));
    let (script, args) = crate::mapping::map_method("farm:raw_trout", &node, None).expect("maps");
    assert_eq!(script, "fishing");
    assert_eq!(args.first().map(String::as_str), Some("raw_trout"), "got {args:?}");
}

#[test]
fn a_pike_quota_needs_the_rod_and_bait() {
    let life = quota("raw_pike", 50);
    let mut node = life.nodes[0].clone();
    resolve_ready(&mut node, &fisher(&["fishing_rod"]));
    assert_eq!(node.status, NodeStatus::Blocked { reason: "missing:fishing_bait".to_string() });
    assert_eq!(crate::replan::need_count(&life.nodes[0], "fishing_bait"), 50, "one bait per pike");
}
