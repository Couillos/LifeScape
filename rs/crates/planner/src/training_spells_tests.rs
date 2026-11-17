use std::collections::HashMap;

use super::{magic_priced_from, spell_options};
use crate::training::{rank_magic, Acquired, TrainPrices};

/// Buy prices of the runes and inputs a test bot can acquire, product
/// values, held units.
fn with_book<R>(buy: &[(&str, f64)], values: &[(&str, f64)], held: &[(&str, i64)], run: impl FnOnce(&TrainPrices) -> R) -> R {
    let buy: HashMap<String, f64> = buy.iter().map(|(name, price)| (name.to_string(), *price)).collect();
    let values: HashMap<String, f64> = values.iter().map(|(name, price)| (name.to_string(), *price)).collect();
    let held_map: HashMap<String, i64> = held.iter().map(|(name, units)| (name.to_string(), *units)).collect();
    let held = |item: &str| held_map.get(item).copied().unwrap_or(0);
    let sale = |_: &str| 0.0;
    let acquire = |item: &str, _: i64| buy.get(item).map(|price| Acquired { coins_each: *price, made_by: None });
    let value = |item: &str, _: i64| values.get(item).copied().unwrap_or(0.0);
    let prices = TrainPrices { coin_rate: 0.5, coins: 100_000, held: &held, sale_value: &sale, acquire: &acquire, product_value: &value, rate: &crate::work_rate::prior_rate };
    run(&prices)
}

#[test]
fn magic_is_priced_from_confuse() {
    assert_eq!(magic_priced_from(), 3);
}

/// A magic climb keeps the runes its D18 options burn between launches:
/// confuse's body rune and the bananas' bones toward 20, law runes once a
/// teleport opens below the target, never before.
#[test]
fn a_magic_climb_keeps_its_spell_runes() {
    let low = crate::training::kept_inputs("magic", 20);
    assert!(low.contains(&"bodyrune".to_string()) && low.contains(&"bones".to_string()), "{low:?}");
    assert!(!low.contains(&"lawrune".to_string()), "no teleport below 25: {low:?}");
    let high = crate::training::kept_inputs("magic", 40);
    assert!(high.contains(&"lawrune".to_string()) && high.contains(&"naturerune".to_string()) && high.contains(&"chaosrune".to_string()), "{high:?}");
    assert!(crate::training::kept_inputs("smithing", 40).is_empty());
}

/// Magic 25, only teleport runes on sale: the Varrock teleport is the
/// pick, its three runes to obtain; a strike nobody sells is out.
#[test]
fn a_teleport_is_priced_with_its_runes() {
    let ranked = with_book(&[("lawrune", 100.0), ("airrune", 5.0), ("firerune", 5.0)], &[], &[], |prices| rank_magic(25, 30, prices, &[]));
    let best = ranked.first().expect("a pick");
    assert_eq!(best.method, "teleport:varrock_teleport", "{ranked:?}");
    let mut missing = best.missing.clone();
    missing.sort();
    assert_eq!(missing, vec!["airrune", "firerune", "lawrune"]);
    // The cast's ticks come from its yield (R3.3), never a constant.
    let ticks = 35.0 * crate::work_rate::prior_rate("teleport:varrock_teleport", 25).and_then(|rate| rate.ticks_per_xp("magic")).expect("teleport rate");
    assert!((best.xp_per_tick - 35.0 / ticks).abs() < 1e-9);
    // Cost per XP: the runes of one cast (100 + 3x5 + 5) and the bot's time.
    assert!((best.cost_per_xp - (120.0 + ticks * 0.5) / 35.0).abs() < 1e-9, "{best:?}");
    assert!(!ranked.iter().any(|pick| pick.method == "teleport:lumbridge_teleport"), "Lumbridge opens at 31");
}

/// An air staff pays the teleport's air runes: never to obtain.
#[test]
fn a_staff_pays_its_element() {
    let ranked = with_book(&[("lawrune", 100.0), ("airrune", 5.0), ("firerune", 5.0)], &[], &[("staff_of_air", 1)], |prices| spell_options(25, 30, prices));
    let varrock = ranked.iter().find(|pick| pick.method == "teleport:varrock_teleport").expect("priced");
    assert!(!varrock.missing.contains(&"airrune".to_string()), "{varrock:?}");
}

/// Curses on the climb's camp: confuse on chickens at 3, weaken on cows
/// at 11 (body, water and earth runes).
#[test]
fn curses_are_priced_on_the_climbs_camp() {
    let runes = [("bodyrune", 6.0), ("waterrune", 5.0), ("earthrune", 5.0)];
    let low = with_book(&runes, &[], &[], |prices| spell_options(3, 5, prices));
    assert!(low.iter().any(|pick| pick.method == "curse:confuse/chickens"), "{low:?}");
    assert!(!low.iter().any(|pick| pick.method.starts_with("curse:weaken")), "weaken opens at 11");
    let mid = with_book(&runes, &[], &[], |prices| spell_options(11, 15, prices));
    let weaken = mid.iter().find(|pick| pick.method == "curse:weaken/cows").expect("weaken priced");
    let ticks_per_xp = crate::work_rate::prior_rate("curse:weaken/cows", 11).and_then(|rate| rate.ticks_per_xp("magic")).expect("curse rate");
    assert!((weaken.xp_per_tick - 1.0 / ticks_per_xp).abs() < 1e-9);
    assert!(weaken.missing.contains(&"bodyrune".to_string()));
}

/// Cheap bones, bananas that sell: bones to bananas pays and wins.
#[test]
fn bananas_win_when_bones_are_cheap_and_bananas_sell() {
    let ranked = with_book(&[("bones", 1.0), ("earthrune", 5.0), ("waterrune", 5.0), ("naturerune", 60.0)], &[("banana", 10.0)], &[], |prices| rank_magic(15, 20, prices, &[]));
    let best = ranked.first().expect("a pick");
    assert_eq!(best.method, "bananas:bones", "{ranked:?}");
    assert!(best.profit_per_tick > 0.0, "{best:?}");
    assert!(best.missing.contains(&"bones".to_string()));
}

/// Crumble undead at 39: priced on the surface undead camp only.
#[test]
fn crumble_undead_is_priced_on_the_undead() {
    let ranked = with_book(&[("chaosrune", 90.0), ("airrune", 5.0), ("earthrune", 5.0)], &[], &[], |prices| spell_options(39, 40, prices));
    assert!(ranked.iter().any(|pick| pick.method == "fight:magic/crumble_undead/wild_skeletons"), "{ranked:?}");
    assert!(!ranked.iter().any(|pick| pick.method.contains("crumble_undead/cows")));
    let young = with_book(&[("chaosrune", 90.0), ("airrune", 5.0), ("earthrune", 5.0)], &[], &[], |prices| spell_options(38, 40, prices));
    assert!(!young.iter().any(|pick| pick.method.contains("crumble_undead")), "crumble opens at 39");
}
