//! Each obtain leg carries its own estimate (live 2026-11-22: the three
//! iron bar legs shared one figure, the ground pickup won the tie).

use crate::cost::LegCost;
use crate::expand::{expand_goal, resolve_ready, ReadyCtx};
use crate::model::{Archetype, GoalKind, Profile};

fn resolved(ground: LegCost, smelt: LegCost) -> (Option<String>, Vec<(String, u64)>) {
    let profile = Profile::new(false, 0.72, 0.4, 7, Archetype::Producer);
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "iron_bar".to_string(), count: 10 }, &profile);
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins_per_tick = 1.0;
    ctx.price = 1000;
    ctx.leg_costs = vec![ground, smelt];
    resolve_ready(&mut nodes[0], &ctx);
    (nodes[0].method.clone(), nodes[0].alternatives.iter().map(|alt| (alt.method.clone(), alt.est_ticks)).collect())
}

fn leg(method: &str, yield_per_6000: f64, input_ticks_per_unit: f64) -> LegCost {
    LegCost { method: method.to_string(), yield_per_6000, input_ticks_per_unit }
}

/// A 555 ticks per bar pickup loses to a smelt that buys its ore.
#[test]
fn smelting_beats_a_slow_ground_pickup() {
    let (method, alternatives) = resolved(leg("ground:iron_bar", 10.8, 0.0), leg("smelt:iron_bar", 1200.0, 6.0));
    let ticks = |name: &str| alternatives.iter().find(|(known, _)| known == name).map(|(_, ticks)| *ticks).expect(name);
    assert!(ticks("smelt:iron_bar") < ticks("ground:iron_bar"), "{alternatives:?}");
    assert_eq!(method.as_deref(), Some("smelt:iron_bar"), "{alternatives:?}");
}

/// A pickup that truly is faster still wins.
#[test]
fn a_faster_pickup_still_wins() {
    let (method, alternatives) = resolved(leg("ground:iron_bar", 3000.0, 0.0), leg("smelt:iron_bar", 1200.0, 900.0));
    assert_eq!(method.as_deref(), Some("ground:iron_bar"), "{alternatives:?}");
}

fn ore_node(farm_yield: f64, price: i64, coins: i64) -> (Option<String>, String, Vec<(String, u64)>) {
    let profile = Profile::new(false, 0.72, 0.4, 7, Archetype::Producer);
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "iron_ore".to_string(), count: 100 }, &profile);
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins_per_tick = 2.0;
    ctx.price = price;
    ctx.coins = coins;
    ctx.bank_known = true;
    ctx.tools = vec!["bronze_pickaxe".to_string()];
    ctx.leg_costs = vec![leg("farm:iron_ore", farm_yield, 0.0)];
    resolve_ready(&mut nodes[0], &ctx);
    let status = format!("{:?}", nodes[0].status);
    (nodes[0].method.clone(), status, nodes[0].alternatives.iter().map(|alt| (alt.method.clone(), alt.est_ticks)).collect())
}

/// Live 2026-11-23: a farm yielding ~nothing ran for 12 h because no
/// leg was ever compared with the purchase. Every leg now competes with
/// `market:buy` (ticks, at the bot's coin rate): the slow farm loses,
/// the bot earns the coins (typed block on coins) instead of farming.
#[test]
fn a_hopeless_farm_loses_to_the_purchase_even_with_an_empty_wallet() {
    let (method, status, alternatives) = ore_node(0.05, 100, 0);
    assert_ne!(method.as_deref(), Some("farm:iron_ore"), "{alternatives:?}");
    assert!(status.contains("missing:coins"), "the coins are earned first: {status}");
}

/// A farm cheaper than the purchase still wins.
#[test]
fn a_fast_farm_still_beats_the_purchase() {
    let (method, _, alternatives) = ore_node(3000.0, 100, 0);
    assert_eq!(method.as_deref(), Some("farm:iron_ore"), "{alternatives:?}");
}

/// Coins are earned, never bought: no market leg on them.
#[test]
fn coins_are_never_compared_with_buying_coins() {
    let profile = Profile::new(false, 0.72, 0.4, 7, Archetype::Producer);
    let mut nodes = expand_goal(&GoalKind::Obtain { item: "coins".to_string(), count: 100 }, &profile);
    let mut ctx = ReadyCtx::blind(0.5);
    ctx.coins_per_tick = 2.0;
    ctx.price = 1;
    ctx.coins = 1000;
    resolve_ready(&mut nodes[0], &ctx);
    assert_ne!(nodes[0].method.as_deref(), Some("market:buy"), "{:?}", nodes[0].alternatives);
}
