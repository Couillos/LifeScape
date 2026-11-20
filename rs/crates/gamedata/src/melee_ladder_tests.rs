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

/// Gates: ten coins open the toll. The hill giants need no key: the
/// Edgeville trapdoor and the main dungeon reach their hall (golden
/// route `geants_sans_cle`); the brass key is only a shortcut.
#[test]
fn gates_follow_the_holdings() {
    let melee = fighter(40, 40, &STEEL);
    let giants = |holdings: &Holdings| rate(&melee, holdings).into_iter().find(|entry| entry.rung.key == "hill_giants").expect("giants");
    assert_eq!(giants(&stocked()).refused, None, "no key: workable by the trapdoor");
    assert_eq!(giants(&Holdings { brass_key: true, ..stocked() }).refused, None, "key held: workable");
    let warriors = |holdings: &Holdings| rate(&melee, holdings).into_iter().find(|entry| entry.rung.key == "al_kharid_warriors").expect("warriors");
    assert!(warriors(&stocked()).refused.is_some(), "west, broke: closed");
    assert_eq!(warriors(&Holdings { coins: 10, ..stocked() }).refused, None, "ten coins pay the toll");
    assert_eq!(warriors(&Holdings { east_of_toll: true, ..stocked() }).refused, None, "already east");
}

/// Live 2026-11-14 (george_97: attack 61, strength 61, defence 60,
/// hitpoints 60, combat 75, black armour, 10 shrimps, no key): the
/// ladder hunted guards forever, the giants refused `gate BrassKey
/// closed`. The strongest workable rungs now hold the band.
#[test]
fn veterans_hunt_the_giants_without_a_key() {
    let melee = Fighter { attack_bonus: [-2, 20, 15], strength_bonus: 24, defence_bonus: [46, 57, 61], ..Fighter::naked(61, 61, 60, 60, 75) };
    let holdings = Holdings { food_count: 10, best_heal: 3, coins: 0, brass_key: false, east_of_toll: false };
    let rated = rate(&melee, &holdings);
    let giants = rated.iter().find(|entry| entry.rung.key == "hill_giants").expect("giants");
    assert_eq!(giants.refused, None, "giants workable: {:?}", giants.refused);
    let mut picked = std::collections::BTreeSet::new();
    for seed in 0..32 {
        picked.insert(picked_key(&choose(&melee, &holdings, seed * 7_919)));
    }
    assert!(picked.contains("hill_giants"), "giants in the band: {picked:?}");
    assert!(!picked.contains("guards"), "guards fell out of the band: {picked:?}");
}

/// Dungeon approach (`approach_threat`): the hunters met between the
/// trapdoor and the hall must not burst the bot from its eat line
/// either. A 15-hitpoint bot survives two giant hits (eat line 8.25)
/// but not the armed hobgoblins on the way down.
#[test]
fn approach_burst_keeps_low_hitpoints_out_of_the_dungeon() {
    let frail = fighter(30, 15, &IRON);
    let giants = rate(&frail, &stocked()).into_iter().find(|entry| entry.rung.key == "hill_giants").expect("giants");
    assert!(giants.refused.as_deref().is_some_and(|why| why.starts_with("approach")), "{:?}", giants.refused);
    let sturdy = fighter(30, 30, &IRON);
    let giants = rate(&sturdy, &stocked()).into_iter().find(|entry| entry.rung.key == "hill_giants").expect("giants");
    assert_eq!(giants.refused, None, "30 hitpoints walk down: {:?}", giants.refused);
    let threat = crate::melee_ladder::approach_threat(&rung_by_key("hill_giants").expect("giants"), (3111, 9837, 0), 20, 8.25);
    assert!(threat.is_some(), "combat 20 meets a biting hunter on the way");
    assert_eq!(crate::melee_ladder::approach_threat(&rung_by_key("cows").expect("cows"), (3253, 3270, 0), 3, 1.0), None, "surface: no approach");
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
/// (rung, first cluster) pair, and one name always lands the same. At
/// melee 30 the band holds guards, hobgoblins and the giants; at 45 only
/// the two strongest camps stay in it (giants and hobgoblins), the fleet
/// still split between them.
#[test]
fn seeds_spread_the_fleet_and_stay_stable() {
    let spread = |melee: Fighter| {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..12 {
            let seed = seed_of(&format!("t{index:02}"));
            if let Some(Pick::Hunt { rung, clusters, .. }) = choose(&melee, &stocked(), seed) {
                seen.insert((rung.key, clusters.first().map(|(x, z, _, _)| (*x, *z))));
            }
        }
        seen
    };
    let seen = spread(fighter(30, 30, &STEEL));
    assert!(seen.len() >= 3, "12 bots at melee 30 spread over {seen:?}");
    let seen = spread(fighter(45, 45, &STEEL));
    assert!(seen.len() >= 2, "12 bots at melee 45 spread over {seen:?}");
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



/// The asked food grows with max hitpoints, never past half of them.
#[test]
fn asked_food_follows_max_hitpoints() {
    assert_eq!(super::food_to_ask(1), "shrimp", "shrimps at the least");
    assert_eq!(super::food_to_ask(10), "herring");
    assert_eq!(super::food_to_ask(20), "tuna");
    assert_eq!(super::food_to_ask(24), "lobster");
    assert_eq!(super::food_to_ask(28), "swordfish");
    assert_eq!(super::food_to_ask(99), "swordfish");
}

/// User 2026-11-17: a foodless fight obtains the best food its wallet
/// buys at the market, never above the hitpoints fit (the asked food);
/// nothing priced and affordable keeps the asked food.
#[test]
fn foodless_fights_ask_the_best_affordable_food() {
    let prices = |food: &str| match food {
        "swordfish" => Some(1_200),
        "lobster" => Some(400),
        "trout" => Some(20),
        "shrimp" => Some(5),
        _ => None,
    };
    assert_eq!(super::affordable_food("swordfish", 20, 30_000, &prices), "swordfish", "rich: the fitted best");
    assert_eq!(super::affordable_food("swordfish", 20, 10_000, &prices), "lobster", "lobsters fit the wallet");
    assert_eq!(super::affordable_food("swordfish", 20, 500, &prices), "trout", "trout at 20 each");
    assert_eq!(super::affordable_food("lobster", 20, 100_000, &prices), "lobster", "never above the fitted heal");
    assert_eq!(super::affordable_food("swordfish", 20, 10, &|_| None), "swordfish", "nothing priced: asked stays");
    assert_eq!(super::affordable_food("cabbage", 20, 10, &prices), "cabbage", "off the ladder: asked stays");
    assert_eq!(super::food_room(20), 10, "a 20-HP bot carries heals up to 10");
}

const ADAMANT: [&str; 5] = ["adamant_scimitar", "adamant_full_helm", "adamant_platebody", "adamant_platelegs", "adamant_kiteshield"];
const RUNE: [&str; 5] = ["rune_scimitar", "rune_full_helm", "rune_platebody", "rune_platelegs", "rune_kiteshield"];

/// Swordfish and the boat's round trip in hand.
fn supplied() -> Holdings {
    Holdings { food_count: 20, best_heal: 14, coins: 500, brass_key: false, east_of_toll: false }
}

fn rated_rung(melee: &Fighter, holdings: &Holdings, key: &str) -> super::Rated {
    rate(melee, holdings).into_iter().find(|entry| entry.rung.key == key).unwrap_or_else(|| panic!("{key} rated"))
}

/// User 2026-11-20: 47 bots at combat 60 and up, nothing above the hill
/// giants. The lesser demons of the Karamja volcano open from combat 80
/// behind the boat (the 60-coin fare out and back), the greater demons
/// of the deep Wilderness from 90.
#[test]
fn high_levels_reach_the_demons() {
    let eighty = fighter(72, 72, &RUNE);
    assert!((80..90).contains(&eighty.combat), "combat {}", eighty.combat);
    let lesser = rated_rung(&eighty, &supplied(), "lesser_demons");
    assert_eq!(lesser.refused, None, "combat {}: {:?}", eighty.combat, lesser.refused);
    assert!(lesser.clusters.iter().all(|(x, z, _, _)| (2820..=2850).contains(x) && (9548..=9570).contains(z)), "the volcano only: {:?}", lesser.clusters);
    let broke = rated_rung(&eighty, &Holdings { coins: 59, ..supplied() }, "lesser_demons");
    assert!(broke.refused.as_deref().is_some_and(|why| why.contains("Boat")), "no round trip: {:?}", broke.refused);
    let greater = rated_rung(&eighty, &supplied(), "greater_demons");
    assert!(greater.refused.is_some(), "greater demons wait for 90: {:?}", greater.refused);
    let ninety = fighter(80, 80, &RUNE);
    assert!(ninety.combat >= 90, "combat {}", ninety.combat);
    let greater = rated_rung(&ninety, &supplied(), "greater_demons");
    assert_eq!(greater.refused, None, "combat {}: {:?}", ninety.combat, greater.refused);
    let seventy = fighter(62, 62, &ADAMANT);
    assert!(seventy.combat < 80);
    assert!(rated_rung(&seventy, &supplied(), "lesser_demons").refused.is_some(), "combat {} stays off the demons", seventy.combat);
    // The band of an 80+ fighter leaves the hill giants behind.
    let mut picked = std::collections::BTreeSet::new();
    for index in 0..24 {
        picked.insert(picked_key(&choose(&eighty, &supplied(), seed_of(&format!("v{index:02}")))));
    }
    assert!(picked.contains("lesser_demons"), "the demons hold the band: {picked:?}");
    assert!(!picked.contains("cows") && !picked.contains("guards"), "{picked:?}");
}

/// Combat 60 to 80: the ice camps (dungeon under Mudskipper Point,
/// Draynor bank; the deep Wilderness camp, one trip).
#[test]
fn mid_high_levels_reach_the_ice_camps() {
    let sixty = fighter(60, 60, &ADAMANT);
    let dungeon = rated_rung(&sixty, &supplied(), "ice_dungeon");
    assert_eq!(dungeon.refused, None, "combat {}: {:?}", sixty.combat, dungeon.refused);
    assert!(dungeon.clusters.iter().all(|(_, z, _, _)| *z > 9500), "{:?}", dungeon.clusters);
    let wild = rated_rung(&sixty, &supplied(), "wild_ice");
    assert_eq!(wild.refused, None, "one trip in the Wilderness: {:?}", wild.refused);
    let weak = fighter(35, 35, &STEEL);
    assert!(rated_rung(&weak, &supplied(), "ice_dungeon").refused.is_some(), "combat {} stays off the ice", weak.combat);
}

/// Item 2 (user 2026-11-20): a camp with no bank within the trek limit
/// is one trip until the food runs out, its danger still a combat
/// requirement (the other hunters, the risk floor), never `NEVER`.
#[test]
fn a_camp_without_a_bank_is_one_trip() {
    let greater = rung_by_key("greater_demons").expect("greater");
    let clusters = rung_clusters(&greater);
    assert!(!clusters.is_empty());
    for (x, z, level, _) in &clusters {
        assert!(!super::cluster_banked(&greater, (*x, *z, *level)), "no bank in reach of ({x},{z})");
        let needed = super::cluster_requirement(&greater, (*x, *z, *level));
        assert!((90..crate::danger::NEVER).contains(&needed), "({x},{z}) needs {needed}");
    }
    let lesser = rung_by_key("lesser_demons").expect("lesser");
    let (x, z, level, _) = rung_clusters(&lesser)[0];
    assert!(super::cluster_banked(&lesser, (x, z, level)), "the volcano banks at Draynor by the boat");
    // The far trip costs fighting time: the same camp with a bank would
    // be worth more.
    let rated = rated_rung(&fighter(80, 80, &RUNE), &supplied(), "greater_demons");
    assert!(rated.value < rated.estimate.xp_per_tick, "{rated:?}");
}

/// Item 4 (user 2026-11-20): archers and mages climb the same camps with
/// their own levels and gear; their defence and hitpoints keep the same
/// safety rules (the foe answers in melee).
#[test]
fn archers_and_mages_climb_the_ladder() {
    let base = Fighter::naked(1, 1, 70, 80, 85).wearing(&gear(&["rune_full_helm", "rune_platebody", "rune_platelegs"]));
    let archer = base.shooting(85, 70, crate::equipment::by_name("rune_arrow").expect("rune arrow").rangebonus);
    let lesser = rated_rung(&archer, &supplied(), "lesser_demons");
    assert_eq!(lesser.refused, None, "{:?}", lesser.refused);
    let pick = picked_key(&choose(&archer, &supplied(), seed_of("archer")));
    assert!(!matches!(pick, "cows" | "chickens"), "a ranged 85 leaves the cows: {pick}");
    let mage = base.casting(85, 20, 16, 34.5);
    let pick = picked_key(&choose(&mage, &supplied(), seed_of("mage")));
    assert!(!matches!(pick, "cows" | "chickens"), "a magic 85 leaves the cows: {pick}");
    // Untrained fists rate the same body far lower.
    let fists = rated_rung(&base, &supplied(), "lesser_demons");
    assert!(fists.estimate.xp_per_tick < lesser.estimate.xp_per_tick / 3.0);
    // Crumble undead reaches no rung's foes but undead ones.
    let unfit = super::unfit_for_spell("crumble_undead");
    assert!(unfit.contains(&"lesser_demons") && unfit.contains(&"cows"), "{unfit:?}");
    assert!(super::unfit_for_spell("fire_blast").is_empty());
}
