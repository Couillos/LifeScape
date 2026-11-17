use super::{per_action_need, tranche_actions, tranche_consumption, tranche_consumption_with, tranche_need, tranche_ticks, tranche_ticks_with, xp_between, xp_per_action};
use crate::work_rate::WorkRate;

#[test]
fn xp_between_reads_the_server_table() {
    assert_eq!(xp_between(1, 2), 83.0);
    assert_eq!(xp_between(1, 10), 1154.0);
    assert_eq!(xp_between(10, 10), 0.0);
}

/// Fishing 20→30 on trout (50 XP a catch): 8 893 XP, 178 catches, one
/// feather each (audit 2026-11-13: consumables were sized at 1 unit).
#[test]
fn bait_scales_with_the_tranche() {
    let actions = tranche_actions("gather:fishing/raw_trout", 20, 30).expect("trout rate");
    let catches = ((133_630.0 - 44_700.0) / 10.0 / 50.0_f64).ceil() as i64;
    assert!(actions >= catches, "an attempt pays at most one catch: {actions} < {catches}");
    let feathers = tranche_need("gather:fishing/raw_trout", 20, 30, "feather").expect("feathers");
    assert!(feathers == catches || feathers == catches + 1, "a feather per catch (the last attempt whole): {feathers} for {catches}");
    assert_eq!(tranche_need("gather:fishing/raw_shrimp", 1, 5, "feather"), None, "nets burn nothing");
}

/// Recipe inputs follow the XP and the odds: cooking shrimps 1→10 (30 XP
/// a cooked one) eats at least 39 raw shrimps, more for the burnt ones;
/// iron smelting fails half the time, so every bar costs two ores;
/// runecraft burns one essence per rune.
#[test]
fn recipe_inputs_follow_the_xp() {
    let raw = tranche_need("cook/raw_shrimp", 1, 10, "raw_shrimp").expect("raw shrimps");
    assert!(raw > 39 && raw < 39 * 3, "39 cooked plus the burnt ones: {raw}");
    let ores = tranche_need("smelt:iron_bar", 15, 20, "iron_ore").expect("iron ore");
    let bars = (xp_between(15, 20) / 12.5).ceil() as i64;
    assert!((ores - 2 * bars).abs() <= 1, "two ores a bar: {ores} for {bars} bars");
    let airs = tranche_actions("runecraft:airrune", 1, 2).expect("air xp");
    assert_eq!(airs, 17, "83 XP at 5 XP an essence");
    assert_eq!(tranche_need("runecraft:airrune", 1, 2, "blankrune"), Some(17));
    assert_eq!(tranche_need("runecraft:airrune", 1, 2, "air_talisman"), None, "the talisman is a tool");
    assert!(tranche_need("jewelry:gold_ring", 5, 8, "gold_bar").is_some_and(|bars| bars > 10));
    assert_eq!(tranche_need("jewelry:gold_ring", 5, 8, "ring_mould"), None, "the mould is kept");
    assert_eq!(per_action_need("smelt:iron_bar", 15, "iron_ore"), Some(1), "one ore an attempt");
}

/// Fights burn ammunition: every shot an arrow, every cast the spell's
/// runes (fire strike: 3 fire, 2 air, 1 mind for each cast).
#[test]
fn fights_burn_arrows_and_runes() {
    let arrows = tranche_need("fight:ranged/cows", 1, 10, "bronze_arrow").expect("arrows");
    assert!(arrows > 250, "1 154 XP at under 4 XP a shot, got {arrows}");
    let runes = tranche_consumption("fight:magic/fire_strike/cows", 13, 20);
    let fire = runes.iter().find(|(rune, _)| rune == "firerune").map(|(_, units)| *units).expect("fire runes");
    let air = runes.iter().find(|(rune, _)| rune == "airrune").map(|(_, units)| *units).expect("air runes");
    let mind = runes.iter().find(|(rune, _)| rune == "mindrune").map(|(_, units)| *units).expect("mind runes");
    assert!((fire - 3 * mind).abs() <= 3 && (air - 2 * mind).abs() <= 2, "3:2:1 a cast: {runes:?}");
    assert!(mind > 0 && (mind as f64) < xp_between(13, 20) / 11.5, "every cast pays the base 11.5 XP at least: {runes:?}");
    assert!(tranche_consumption("fight:melee/cows", 1, 10).is_empty(), "melee burns nothing");
}

/// R3.3: XP per action, ticks and consumption come from the rate read,
/// never a constant: a measured rate twice as fast halves the ticks and
/// keeps the units per XP.
#[test]
fn a_measured_rate_drives_ticks_and_consumption() {
    let measured = |_: &str, _: u8| Some(WorkRate { actions_6000: 600.0, xp_6000: vec![("cooking".to_string(), 18_000.0)], flows_6000: vec![("raw_shrimp".to_string(), "input".to_string(), -600.0), ("shrimp".to_string(), "product".to_string(), 600.0)] });
    let faster = |_: &str, _: u8| Some(WorkRate { actions_6000: 1200.0, xp_6000: vec![("cooking".to_string(), 36_000.0)], flows_6000: vec![("raw_shrimp".to_string(), "input".to_string(), -1200.0)] });
    let slow = tranche_ticks_with(&measured, "cook/raw_shrimp", 1, 10).expect("ticks");
    let fast = tranche_ticks_with(&faster, "cook/raw_shrimp", 1, 10).expect("ticks");
    assert!((slow - 1154.0 / 3.0).abs() < 1e-6, "1 154 XP at 3 XP a tick: {slow}");
    assert!((slow - 2.0 * fast).abs() < 1e-6);
    assert_eq!(tranche_consumption_with(&measured, "cook/raw_shrimp", 1, 10), vec![("raw_shrimp".to_string(), 39)], "one raw shrimp per 30 XP");
    assert!(tranche_ticks_with(&|_: &str, _: u8| None, "cook/raw_shrimp", 1, 10).is_none(), "no rate, no duration");
    assert!(tranche_ticks("cook/raw_shrimp", 1, 10).is_some_and(|ticks| ticks > 0.0), "the game prior");
    assert!(xp_per_action("fight:melee/cows", 1).is_none(), "a melee fight's skill rides in its node");
}

/// R7 (b): a block's obtain counts read the reader the brain hands
/// (`replan::need_count_with`): a measured rate twice as hungry in
/// feathers asks twice the feathers of the prior.
#[test]
fn block_counts_read_the_measured_reader() {
    let hungry = |method: &str, level: u8| {
        crate::work_rate::prior_rate(method, level).map(|mut rate| {
            for flow in &mut rate.flows_6000 {
                if flow.0 == "feather" {
                    flow.2 *= 2.0;
                }
            }
            rate
        })
    };
    let mut node = crate::model::Node::new("n1", crate::model::NodeKind::ReachLevel { skill: "fishing".to_string(), level: 30 }, Vec::new(), 30);
    node.method = Some("gather:fishing/raw_trout".to_string());
    let prior = crate::replan::need_count_from(&node, "feather", Some(20));
    let measured = crate::replan::need_count_with(&node, "feather", Some(20), &hungry);
    assert!(prior > 100 && (measured - 2 * prior).abs() <= 1, "measured {measured} vs prior {prior}");
    assert_eq!(super::tranche_need_with(&hungry, "gather:fishing/raw_trout", 20, 30, "feather"), Some(measured));
}
