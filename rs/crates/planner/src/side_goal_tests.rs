//! Side goals: no bot idles while its milestones' skills are banned.

use super::{method_banned, side_goal, steer_off_bans, SIDE_PREFIX};
use crate::ambition_catalog::DemandItem;
use crate::expand::ReadyCtx;
use crate::model::{Alternative, GoalKind, Node, NodeKind};

fn banned(skills: &[&str]) -> Vec<String> {
    skills.iter().map(|skill| skill.to_string()).collect()
}

fn bid(item: &str, weight: f64, margin: f64) -> DemandItem {
    DemandItem { item: item.to_string(), weight, unmet: 40, margin: Some(margin) }
}

fn nothing_held(_: &str) -> i64 {
    0
}

/// Live 2026-11-13: "every open milestone trains a banned skill
/// (cooking,woodcutting)" idled the bot for 6000 ticks. The heaviest
/// paying demand it can make with a free skill becomes a side goal; a
/// banned skill, an unmet level, a loss or a quota already held never.
#[test]
fn a_banned_bot_serves_the_best_paying_demand_it_can_make() {
    let avoid = banned(&["cooking", "woodcutting"]);
    let fresh = |_: &str| 1u8;
    let demand = vec![
        bid("shrimp", 900.0, 5.0),          // cooking: banned
        bid("logs", 800.0, 3.0),            // woodcutting: banned
        bid("steel_platebody", 700.0, -2.0), // a loss
        bid("bronze_platebody", 600.0, 4.0), // smithing 18: level not met
        bid("bronze_dagger", 100.0, 2.0),    // smithing 1: workable
    ];
    let (goal, origin) = side_goal(&fresh, &avoid, &demand, &nothing_held, false).expect("a side goal");
    assert_eq!(goal, GoalKind::Produce { item: "bronze_dagger".to_string(), count: 40 });
    assert_eq!(origin, format!("{SIDE_PREFIX}demand:bronze_dagger"));
    let stocked = |item: &str| if item == "bronze_dagger" { 40 } else { 0 };
    let (goal, origin) = side_goal(&fresh, &avoid, &demand, &stocked, false).expect("money then");
    assert_eq!((goal, origin.as_str()), (GoalKind::MakeMoney, "side:money"), "quota already held: no production");
}

/// No paying demand: a money goal; Iron never serves the market; with
/// every money skill banned an Iron has nothing (typed by the caller).
#[test]
fn without_demand_the_side_goal_makes_money() {
    let fresh = |_: &str| 1u8;
    let avoid = banned(&["cooking"]);
    assert_eq!(side_goal(&fresh, &avoid, &[], &nothing_held, false).map(|(goal, _)| goal), Some(GoalKind::MakeMoney));
    let paying = vec![bid("copper_ore", 100.0, 2.0)];
    assert_eq!(side_goal(&fresh, &avoid, &paying, &nothing_held, true).map(|(goal, _)| goal), Some(GoalKind::MakeMoney), "Iron: money only");
    let everything = banned(&["fishing", "woodcutting", "mining", "crafting", "strength"]);
    assert_eq!(side_goal(&fresh, &everything, &[], &nothing_held, true), None);
    assert!(side_goal(&fresh, &everything, &[], &nothing_held, false).is_some(), "the flip leg needs no skill");
}

/// Melee is one group: a strength ban bans every fight method.
#[test]
fn bans_read_methods_by_skill_group() {
    assert!(method_banned("fight:melee/cows", &banned(&["strength"])));
    assert!(method_banned("gather:fishing/raw_shrimp", &banned(&["fishing"])));
    assert!(!method_banned("gather:fishing/raw_shrimp", &banned(&["woodcutting"])));
    assert!(!method_banned("market:flip", &banned(&["fishing", "attack"])));
}

/// A money node resolved onto a banned skill moves to the next ranked
/// method off the bans; all banned is a typed refusal.
#[test]
fn money_resolution_steers_off_banned_skills() {
    let mut node = Node::new("n1", NodeKind::MakeMoney, Vec::new(), 0);
    for method in ["gather:woodcut/logs", "market:flip"] {
        node.alternatives.push(Alternative { method: method.to_string(), est_ticks: 6000, est_coins: 0 });
    }
    node.method = Some("gather:woodcut/logs".to_string());
    let ctx = ReadyCtx::blind(0.5);
    assert_eq!(steer_off_bans(&mut node, &ctx, &banned(&["fishing"])), Ok(false), "not banned: untouched");
    assert_eq!(steer_off_bans(&mut node, &ctx, &banned(&["woodcutting"])), Ok(true));
    assert_eq!(node.method.as_deref(), Some("market:flip"));
    node.alternatives.retain(|alt| alt.method != "market:flip");
    node.method = Some("gather:woodcut/logs".to_string());
    assert!(steer_off_bans(&mut node, &ctx, &banned(&["woodcutting"])).is_err());
}
