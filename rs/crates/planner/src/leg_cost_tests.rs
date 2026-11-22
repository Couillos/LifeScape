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
