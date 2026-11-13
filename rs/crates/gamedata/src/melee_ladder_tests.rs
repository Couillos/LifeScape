use super::{choose, food_for, order_clusters, rate, rung_by_key, rung_clusters, seed_of, Holdings, Pick, MAX_FOOD, MIN_FOOD, RUNGS};
use crate::melee_math::Fighter;

/// Gear by debugname (test fixtures only).
fn gear(names: &[&str]) -> Vec<crate::equipment::Wearable> {
    names.iter().map(|name| crate::equipment::by_name(name).unwrap_or_else(|| panic!("{name} wearable"))).collect()
}

/// Combat level from melee levels, hitpoints and prayer 1 (Player.ts).
fn combat_of(attack: i64, strength: i64, defence: i64, hitpoints: i64) -> i64 {
    (0.25 * (defence + hitpoints) as f64 + 0.325 * (attack + strength) as f64) as i64
}

fn fighter(level: i64, hitpoints: i64, kit: &[&str]) -> Fighter {
    Fighter::naked(level, level, level, hitpoints, combat_of(level, level, level, hitpoints)).wearing(&gear(kit))
}

const IRON: [&str; 5] = ["iron_scimitar", "iron_full_helm", "iron_platebody", "iron_platelegs", "iron_kiteshield"];
const STEEL: [&str; 5] = ["steel_scimitar", "steel_full_helm", "steel_platebody", "steel_platelegs", "steel_kiteshield"];

fn stocked() -> Holdings {
    Holdings { food_count: 20, best_heal: 7, coins: 0, brass_key: false, east_of_toll: false }
}

fn picked_key(pick: &Option<Pick>) -> &'static str {
    match pick {
        Some(Pick::Hunt { rung, .. }) => rung.key,
        Some(Pick::NeedFood { rung, .. }) => rung.key,
        None => "none",
    }
}

/// Cows and chickens are the only food-exempt rungs; every other one
/// sizes a load inside the bounds.
#[test]
fn only_cows_and_chickens_skip_food() {
    let exempt: Vec<&str> = RUNGS.iter().filter(|entry| entry.food_exempt).map(|entry| entry.key).collect();
    assert_eq!(exempt, vec!["chickens", "cows"]);
    for entry in rate(&fighter(40, 40, &STEEL), &stocked()) {
        if entry.rung.food_exempt {
            assert_eq!(entry.food, 0, "{} carries no sized load", entry.rung.key);
        } else {
            assert!((MIN_FOOD..=MAX_FOOD).contains(&entry.food), "{} food {}", entry.rung.key, entry.food);
        }
    }
}

/// Every rung names a real combat spot whose NPC has a combat block.
#[test]
fn every_rung_is_a_spot_with_stats() {
    for entry in RUNGS {
        let spot = crate::combat_spots::by_key(entry.key).unwrap_or_else(|| panic!("{} is a combat spot", entry.key));
        assert!(spot.npc_names.contains(&entry.npc), "{} rates one of its own NPCs", entry.key);
        assert!(crate::npc_combat::by_name(entry.npc).is_some(), "{} has a combat block", entry.npc);
        assert!(!rung_clusters(&entry).is_empty(), "{} has an F2P cluster", entry.key);
    }
    let giants = rung_clusters(&rung_by_key("hill_giants").expect("giants"));
    assert!(giants.iter().all(|(_, z, _, _)| *z > 9800), "Edgeville dungeon only, got {giants:?}");
    let hobgoblins = rung_clusters(&rung_by_key("hobgoblins").expect("hobgoblins"));
    assert!(hobgoblins.iter().all(|(x, _, _, _)| *x >= 2895), "peninsula only, got {hobgoblins:?}");
}

/// Food sizing grows with the burn and stays inside the pack bounds.
#[test]
fn food_scales_with_the_burn() {
    let soft = rate(&fighter(40, 40, &STEEL), &stocked()).into_iter().find(|entry| entry.rung.key == "barbarians").expect("barbarians");
    let hard = rate(&fighter(20, 20, &[]), &stocked()).into_iter().find(|entry| entry.rung.key == "guards").expect("guards");
    assert!(food_for(&hard.estimate, 3) >= food_for(&soft.estimate, 3), "harder foe, more food");
    assert!(food_for(&hard.estimate, 3) >= food_for(&hard.estimate, 12), "better food, fewer units");
    assert_eq!(food_for(&soft.estimate, 3).max(MIN_FOOD), food_for(&soft.estimate, 3));
    assert!(food_for(&hard.estimate, 1) <= MAX_FOOD);
}

/// A fresh account (hitpoints 10, no gear, no food) stays on the
/// exempt rungs: anything that bites would need food.
#[test]
fn fresh_bots_stay_on_exempt_rungs() {
    let broke = Holdings { food_count: 0, best_heal: 0, coins: 0, brass_key: false, east_of_toll: false };
    for seed in 0..8 {
        let pick = choose(&fighter(1, 10, &[]), &broke, seed);
        assert!(matches!(picked_key(&pick), "chickens" | "cows"), "seed {seed}: {pick:?}");
    }
}

/// The audit case: melee 45 with steel and food never trains on cows
/// (8 bots crowded the Lumbridge field at ~1 300 xp per 6000 ticks).
#[test]
fn mid_levels_leave_the_cow_field() {
    let rated = rate(&fighter(45, 45, &STEEL), &stocked());
    let cows = rated.iter().find(|entry| entry.rung.key == "cows").expect("cows").value;
    for seed in 0..16 {
        let pick = choose(&fighter(45, 45, &STEEL), &stocked(), seed);
        let key = picked_key(&pick);
        assert!(!matches!(key, "cows" | "chickens"), "seed {seed} picked {key}");
        let value = rated.iter().find(|entry| entry.rung.key == key).expect("rated").value;
        assert!(value > cows * 1.5, "{key} {value:.3} vs cows {cows:.3}");
        assert!(matches!(pick, Some(Pick::Hunt { food: Some(count), .. }) if count >= MIN_FOOD), "food sized: {pick:?}");
    }
}

/// No food at all and the band needs it: a typed food errand, never a
/// hunt that bites without food.
#[test]
fn foodless_mid_levels_ask_for_food() {
    let broke = Holdings { food_count: 0, best_heal: 0, coins: 0, brass_key: false, east_of_toll: false };
    match choose(&fighter(45, 45, &STEEL), &broke, 3) {
        Some(Pick::NeedFood { rung, count }) => {
            assert!(!rung.food_exempt, "{} is exempt", rung.key);
            assert!(count >= MIN_FOOD);
        }
        other => panic!("foodless 45 needs food, got {other:?}"),
    }
}

/// Gates: the brass key opens the hill giants, ten coins the toll.
#[test]
fn gates_follow_the_holdings() {
    let melee = fighter(40, 40, &STEEL);
    let giants = |holdings: &Holdings| rate(&melee, holdings).into_iter().find(|entry| entry.rung.key == "hill_giants").expect("giants");
    assert!(giants(&stocked()).refused.as_deref().is_some_and(|why| why.contains("BrassKey")));
    let keyed = Holdings { brass_key: true, ..stocked() };
    assert_eq!(giants(&keyed).refused, None, "key held: workable");
    let warriors = |holdings: &Holdings| rate(&melee, holdings).into_iter().find(|entry| entry.rung.key == "al_kharid_warriors").expect("warriors");
    assert!(warriors(&stocked()).refused.is_some(), "west, broke: closed");
    assert_eq!(warriors(&Holdings { coins: 10, ..stocked() }).refused, None, "ten coins pay the toll");
    assert_eq!(warriors(&Holdings { east_of_toll: true, ..stocked() }).refused, None, "already east");
}

/// The burst rule: two guard max hits from the eat line of a 10-hp bot
/// could kill it.
#[test]
fn burst_rule_keeps_low_hitpoints_off_hard_hitters() {
    let rated = rate(&fighter(10, 10, &[]), &stocked());
    let guards = rated.iter().find(|entry| entry.rung.key == "guards").expect("guards");
    assert!(guards.refused.as_deref().is_some_and(|why| why.starts_with("burst")), "{:?}", guards.refused);
}

/// Coins break ties: a dropper's value carries its coins on top of its
/// xp (guards 9.7 coins a kill).
#[test]
fn coin_droppers_carry_a_bonus() {
    let rated = rate(&fighter(40, 40, &STEEL), &stocked());
    let guards = rated.iter().find(|entry| entry.rung.key == "guards").expect("guards");
    assert!(guards.coins_per_kill > 9.0, "guards drop coins: {}", guards.coins_per_kill);
    assert!(guards.value > guards.estimate.xp_per_tick * 0.8, "value keeps the xp scale");
    let cows = rated.iter().find(|entry| entry.rung.key == "cows").expect("cows");
    assert_eq!(cows.coins_per_kill, 0.0);
}

/// The seed spreads a fleet: many account names land on more than one
/// (rung, first cluster) pair, and one name always lands the same.
#[test]
fn seeds_spread_the_fleet_and_stay_stable() {
    let melee = fighter(45, 45, &STEEL);
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..12 {
        let seed = seed_of(&format!("t{index:02}"));
        if let Some(Pick::Hunt { rung, clusters, .. }) = choose(&melee, &stocked(), seed) {
            seen.insert((rung.key, clusters.first().map(|(x, z, _, _)| (*x, *z))));
        }
    }
    assert!(seen.len() >= 3, "12 bots spread over {seen:?}");
    assert_eq!(seed_of("t01"), seed_of("t01"));
    let clusters = vec![(1, 1, 0, 3), (50, 50, 0, 3), (5, 5, 0, 2)];
    let firsts: std::collections::BTreeSet<(i64, i64, i64, usize)> = (0..40u64).map(|seed| order_clusters(&clusters, seed * 25_000)[0]).collect();
    assert_eq!(firsts.len(), 3, "seeds start on every cluster: {firsts:?}");
    let ordered = order_clusters(&clusters, 7);
    assert_eq!(ordered, order_clusters(&clusters, 7), "one seed, one order");
    assert_eq!(ordered.len(), 3, "every cluster still visited");
}

/// The ladder table of the report: levels 1 to 60 (iron kit from 10,
/// steel from 30), 20 trout, brass key and coins held. `cargo test -p
/// lifescape-gamedata ladder_table -- --nocapture` prints it.
#[test]
fn ladder_table() {
    let holdings = Holdings { food_count: 20, best_heal: 7, coins: 100, brass_key: true, east_of_toll: false };
    let mut previous = 0.0f64;
    for level in [1i64, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60] {
        let hitpoints = level.max(10);
        let kit: &[&str] = if level >= 30 { &STEEL } else if level >= 10 { &IRON } else { &[] };
        let melee = fighter(level, hitpoints, kit);
        let rated = rate(&melee, &holdings);
        let workable: Vec<String> = rated
            .iter()
            .filter(|entry| entry.refused.is_none())
            .map(|entry| format!("{}={:.2}xp/{:.2}c/{}f", entry.rung.key, entry.estimate.xp_per_tick, entry.coins_per_kill, entry.food))
            .collect();
        let best = rated.iter().filter(|entry| entry.refused.is_none()).map(|entry| entry.value).fold(0.0, f64::max);
        println!("lvl {level:2} hp {hitpoints:2} cb {:2}: best {best:.2} | {}", melee.combat, workable.join(" "));
        assert!(best >= previous * 0.95, "the ladder never drops much as levels rise ({best} after {previous})");
        previous = best;
    }
}

