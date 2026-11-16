//! Server success odds of gathering and cooking (dump
//! `action_odds.json`, `orchestrator dump-npc-ranges` /
//! `dump_action_odds`): the `(low, high)` pairs the scripts hand to
//! `stat_random`, the pickaxe swing rates, the cooking burn pairs.
//!
//! `stat_random(stat, low, high)` (engine `PlayerOps.ts`):
//! `value = floor(low × (99 − L) / 98) + floor(high × (L − 1) / 98) + 1`
//! with `L = min(level, 99)`, success when `value > random(256)`: odds
//! `min(value, 256) / 256`.

use std::collections::HashMap;
use std::sync::OnceLock;

const RAW: &str = include_str!("../../../../gamedata/data/action_odds.json");

/// Odds of one `stat_random(level, low, high)` roll.
pub fn stat_odds(level: i64, low: i64, high: i64) -> f64 {
    let clamped = level.clamp(1, 99);
    let value = (low * (99 - clamped)).div_euclid(98) + (high * (clamped - 1)).div_euclid(98) + 1;
    (value.clamp(0, 256) as f64) / 256.0
}

/// Cooking row of one uncooked object.
#[derive(Debug, Clone, PartialEq)]
pub struct CookOdds {
    pub cooked: String,
    pub burnt: Option<String>,
    /// `successchance` (fires and the default).
    pub fire: (i64, i64),
    /// `successchance_range` when the range differs.
    pub range: Option<(i64, i64)>,
}

#[derive(Default)]
struct Tables {
    rocks: HashMap<String, (i64, i64)>,
    pickaxes: HashMap<String, i64>,
    trees: HashMap<String, HashMap<String, (i64, i64)>>,
    fish: HashMap<String, (i64, i64)>,
    cooking: HashMap<String, CookOdds>,
    runecraft: HashMap<String, i64>,
}

fn pair(value: &lifescape_json::Value) -> Option<(i64, i64)> {
    let parts = value.as_arr()?;
    Some((parts.first()?.as_i64()?, parts.get(1)?.as_i64()?))
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut out = Tables::default();
        let Ok(doc) = lifescape_json::parse(RAW) else { return out };
        for (name, value) in doc.get("rocks").as_obj().cloned().unwrap_or_default() {
            out.rocks.extend(pair(&value).map(|odds| (name.clone(), odds)));
        }
        for (name, value) in doc.get("pickaxes").as_obj().cloned().unwrap_or_default() {
            out.pickaxes.extend(value.as_i64().map(|rate| (name.clone(), rate)));
        }
        for (name, axes) in doc.get("trees").as_obj().cloned().unwrap_or_default() {
            let row = axes.as_obj().cloned().unwrap_or_default().iter().filter_map(|(axe, value)| pair(value).map(|odds| (axe.clone(), odds))).collect();
            out.trees.insert(name, row);
        }
        for (name, value) in doc.get("fish").as_obj().cloned().unwrap_or_default() {
            out.fish.extend(pair(&value).map(|odds| (name.clone(), odds)));
        }
        for (name, value) in doc.get("cooking").as_obj().cloned().unwrap_or_default() {
            let (Some(cooked), Some(fire)) = (value.get("cooked").as_str(), pair(value.get("fire"))) else { continue };
            let odds = CookOdds { cooked: cooked.to_string(), burnt: value.get("burnt").as_str().map(str::to_string), fire, range: pair(value.get("range")) };
            out.cooking.insert(name, odds);
        }
        for (name, value) in doc.get("runecraft").as_obj().cloned().unwrap_or_default() {
            out.runecraft.extend(value.as_i64().map(|multiplier| (name.clone(), multiplier)));
        }
        out
    })
}

/// `rock_successchance` of the rock yielding `product`.
pub fn rock_chance(product: &str) -> Option<(i64, i64)> {
    tables().rocks.get(product).copied()
}

/// `mining_rate` of a pickaxe: one swing every `rate + 1` ticks.
pub fn pickaxe_rate(pickaxe: &str) -> Option<i64> {
    tables().pickaxes.get(pickaxe).copied()
}

/// `successchance` of the tree yielding `product` with `axe`.
pub fn tree_chance(product: &str, axe: &str) -> Option<(i64, i64)> {
    tables().trees.get(product)?.get(axe).copied()
}

/// `success_low`, `success_high` of a raw fish.
pub fn fish_chance(product: &str) -> Option<(i64, i64)> {
    tables().fish.get(product).copied()
}

/// Cooking row of an uncooked object.
pub fn cooking(uncooked: &str) -> Option<CookOdds> {
    tables().cooking.get(uncooked).cloned()
}

/// Cooking row whose cooked product is `cooked`.
pub fn cooking_of(cooked: &str) -> Option<(String, CookOdds)> {
    tables().cooking.iter().find(|(_, odds)| odds.cooked == cooked).map(|(raw, odds)| (raw.clone(), odds.clone()))
}

/// Runes one essence crafts at `level` (`runecraft.rs2`:
/// floor(level / multiplier) + 1), `None` for an unknown rune.
pub fn runes_per_essence(rune: &str, level: i64) -> Option<i64> {
    let multiplier = *tables().runecraft.get(rune)?;
    (multiplier > 0).then(|| level.max(1) / multiplier + 1)
}

#[cfg(test)]
mod tests {
    use super::{cooking, fish_chance, pickaxe_rate, rock_chance, stat_odds, tree_chance};

    #[test]
    fn odds_follow_the_engine_formula() {
        // Copper 128/400 at level 1: floor(128 × 98 / 98) + 0 + 1 = 129.
        assert!((stat_odds(1, 128, 400) - 129.0 / 256.0).abs() < 1e-9);
        // Shrimps 48/256 at 99: 0 + 256 + 1, capped at certain.
        assert_eq!(stat_odds(99, 48, 256), 1.0);
        assert!(stat_odds(30, 128, 400) > stat_odds(10, 128, 400));
    }

    #[test]
    fn tables_carry_the_server_rows() {
        assert_eq!(rock_chance("copper_ore"), Some((128, 400)));
        assert_eq!(pickaxe_rate("bronze_pickaxe"), Some(7));
        assert_eq!(pickaxe_rate("rune_pickaxe"), Some(2));
        assert_eq!(tree_chance("logs", "bronze_axe"), Some((64, 200)));
        assert_eq!(fish_chance("raw_shrimp"), Some((48, 256)));
        assert_eq!(cooking("raw_swordfish").and_then(|row| row.range), Some((30, 310)));
        assert_eq!(super::runes_per_essence("airrune", 10), Some(1));
        assert_eq!(super::runes_per_essence("airrune", 22), Some(3));
    }
}
