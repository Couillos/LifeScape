//! A1 (user 2026-11-21): training as an investment.

use super::*;

fn price(name: &str) -> Option<f64> {
    match name {
        "cosmicrune" => Some(150.0),
        "earthrune" => Some(5.0),
        _ => None,
    }
}

fn rate(method: &str, level: u8) -> Option<WorkRate> {
    crate::work_rate::prior_rate(method, level)
}

/// A climb already done costs no tick; one level costs XP over the best
/// rate; no climb past the horizon is worth anything.
#[test]
fn a_reached_gate_costs_nothing_and_a_climb_costs_its_xp() {
    let constant = |_: &str, _: u8| Some(10.0);
    assert_eq!(climb_ticks("magic", 60, 55, &constant), Some(0.0));
    let ticks = climb_ticks("magic", 50, 55, &constant).expect("a climb");
    assert!((ticks - crate::sizing::xp_between(50, 55) / 10.0).abs() < 1e-6, "{ticks}");
    assert_eq!(climb_ticks("magic", 1, 55, &|_: &str, _: u8| None), None, "no method trains it: typed none, never a default");
}

/// The unpriced inputs of an amulet bring the crafting steps; a priced
/// chain brings none (bought).
#[test]
fn only_the_unpriced_inputs_bring_gates() {
    let level_of = |_: &str| 1u8;
    let xp = |skill: &str, level: u8| best_xp_rate(skill, level);
    let made = investment("amulet_of_power", 1_000_000.0, 66.0, &[], &level_of, &price, &xp, &rate).expect("made");
    assert!(made.gates.iter().any(|gate| gate.skill == "magic" && gate.level == 57), "{made:?}");
    assert!(made.gates.iter().any(|gate| gate.skill == "crafting" && gate.level >= 70), "the unstrung amulet is crafted: {made:?}");
    let bought = |name: &str| price(name).or(if name == "strung_diamond_amulet" { Some(5_000.0) } else { None });
    let buying = investment("amulet_of_power", 1_000_000.0, 66.0, &[], &level_of, &bought, &xp, &rate).expect("buying");
    assert!(buying.gates.iter().all(|gate| gate.skill == "magic"), "the strung amulet is bought: {buying:?}");
    assert!(buying.climb_ticks < made.climb_ticks);
}

/// The pull a pending climb puts on the time value is its value per tick.
#[test]
fn the_pull_is_the_best_value_per_tick() {
    let mut cheap = investment("logs", 4.0, 100.0, &[], &|_: &str| 99, &price, &|skill: &str, level: u8| best_xp_rate(skill, level), &rate).expect("logs");
    cheap.value_6000 = 600.0;
    let mut rich = cheap.clone();
    rich.value_6000 = 6000.0;
    assert!((pull_per_tick(&[cheap, rich]) - 1.0).abs() < 1e-9);
    assert_eq!(pull_per_tick(&[]), 0.0);
}

/// The price table of an objective lists the object and its inputs.
#[test]
fn the_priced_items_list_the_chain() {
    let names = priced_items("amulet_of_power");
    for needed in ["amulet_of_power", "strung_diamond_amulet", "cosmicrune", "unstrung_diamond_amulet"] {
        assert!(names.iter().any(|name| name == needed), "{needed} in {names:?}");
    }
}
