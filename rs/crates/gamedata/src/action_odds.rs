//! Server success odds of gathering and cooking (dump
//! `action_odds.json`, `orchestrator dump-npc-ranges` /
//! `dump_action_odds`): the `(low, high)` pairs the scripts hand to
//! `stat_random`, the pickaxe swing rates, the cooking burn pairs, the
//! gem roll of a mined rock and its `mining_gem_table`.
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

/// The gem roll of a mined rock (`mining.rs2` `get_ore_normal` /
/// `get_ore_fast`, F2P chance) and `mining_gem_table`: one roll in
/// `roll` finds a gem instead of ore, drawn over `draws` (each gem's
/// share of them; the rest finds nothing).
#[derive(Debug, Clone, PartialEq)]
pub struct MiningGems {
    pub roll: i64,
    pub draws: i64,
    pub gems: Vec<(String, i64)>,
}

#[derive(Default)]
struct Tables {
    mining_gems: Option<MiningGems>,
    rocks: HashMap<String, (i64, i64)>,
    rock_respawn: HashMap<String, i64>,
    tree_respawn: HashMap<String, i64>,
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
        for (name, value) in doc.get("rock_respawn").as_obj().cloned().unwrap_or_default() {
            out.rock_respawn.extend(value.as_i64().map(|ticks| (name.clone(), ticks)));
        }
        for (name, value) in doc.get("tree_respawn").as_obj().cloned().unwrap_or_default() {
            out.tree_respawn.extend(value.as_i64().map(|ticks| (name.clone(), ticks)));
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
        let gems = doc.get("mining_gems");
        if let (Some(roll), Some(draws), Some(table)) = (gems.get("roll").as_i64(), gems.get("draws").as_i64(), gems.get("gems").as_obj()) {
            let gems: Vec<(String, i64)> = table.iter().filter_map(|(gem, share)| share.as_i64().map(|share| (gem.clone(), share))).collect();
            if roll > 0 && draws > 0 && !gems.is_empty() {
                out.mining_gems = Some(MiningGems { roll, draws, gems });
            }
        }
        out
    })
}

/// `rock_successchance` of the rock yielding `product`.
pub fn rock_chance(product: &str) -> Option<(i64, i64)> {
    tables().rocks.get(product).copied()
}

/// Ticks a rock yielding `product` stays empty once mined
/// (`rock_respawnrate`, `mining.rs2` `loc_change`); `None` for a rock
/// that never depletes (rune essence).
pub fn rock_respawn(product: &str) -> Option<i64> {
    tables().rock_respawn.get(product).copied()
}

/// Felling of the tree yielding `product` (`woodcutting.rs2`
/// `get_logs`): (odds one log fells it, mean ticks it stays a stump).
/// A normal tree (`respawnrate` 0) falls with every log and is back
/// after 120 + random(80) ticks; any other falls with 1 log in 8.
pub fn tree_felling(product: &str) -> Option<(f64, f64)> {
    let respawn = *tables().tree_respawn.get(product)?;
    Some(if respawn == 0 { (1.0, NORMAL_TREE_RESPAWN_MEAN) } else { (1.0 / TREE_DEPLETE_ONE_IN, respawn as f64) })
}

/// Mean stump ticks of a normal tree: 120 + random(80).
pub const NORMAL_TREE_RESPAWN_MEAN: f64 = 120.0 + 79.0 / 2.0;
/// A non-normal tree falls with one log in 8 (`$deplete_chance = 8`).
pub const TREE_DEPLETE_ONE_IN: f64 = 8.0;

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

/// The gem roll of a mined rock and its table (dump `mining_gems`),
/// `None` when the dump carries none.
pub fn mining_gems() -> Option<MiningGems> {
    tables().mining_gems.clone()
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
        assert_eq!(super::rock_respawn("silver_ore"), Some(200));
        assert_eq!(super::rock_respawn("blankrune"), None, "essence never depletes");
        assert_eq!(super::tree_felling("logs"), Some((1.0, 159.5)));
        assert_eq!(super::tree_felling("yew_logs"), Some((0.125, 200.0)));
    }

    /// The dumped gem table is the server's (`mining.rs2`, read here
    /// again): `random($chance) = ^true` with `$chance` 256 for an F2P
    /// player, `mining_gem_table` over `random(128)` with cumulative
    /// thresholds (diamond < 2, ruby < 10, emerald < 26, sapphire < 58).
    #[test]
    fn mining_gems_are_the_server_script() {
        let gems = super::mining_gems().expect("dump carries mining_gems");
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script = manifest
            .ancestors()
            .map(|dir| dir.join("lostcity/content/scripts/skill_mining/scripts/mining.rs2"))
            .find(|candidate| candidate.is_file())
            .expect("../lostcity checkout next to LifeScape");
        let text = std::fs::read_to_string(&script).expect("mining.rs2 readable");
        let normal = text.split("[label,get_ore_normal]").nth(1).and_then(|rest| rest.split("\n[").next()).expect("get_ore_normal");
        assert!(normal.contains(&format!("def_int $chance = {};", gems.roll)) && normal.contains("random($chance) = ^true"), "gem roll moved in the server script");
        let fast = text.split("[label,get_ore_fast]").nth(1).and_then(|rest| rest.split("\n[").next()).expect("get_ore_fast");
        assert!(fast.contains(&format!("def_int $chance = {};", gems.roll)), "the fast rocks roll the same gems");
        let table = text.split("[proc,mining_gem_table]").nth(1).expect("mining_gem_table");
        assert!(table.contains(&format!("random({});", gems.draws)), "draws moved");
        let mut threshold = 0;
        let mut order: Vec<(String, i64)> = Vec::new();
        for line in table.lines().map(str::trim).filter(|line| line.starts_with("if ($rand < ")) {
            let (bound, gem) = line.trim_start_matches("if ($rand < ").split_once(") return (").expect("threshold line");
            let bound: i64 = bound.parse().expect("threshold");
            order.push((gem.trim_end_matches(");").to_string(), bound - threshold));
            threshold = bound;
        }
        order.sort();
        let mut dumped = gems.gems.clone();
        dumped.sort();
        assert_eq!(dumped, order, "the dump is the server table");
        assert_eq!(dumped.iter().find(|(gem, _)| gem == "uncut_sapphire").map(|(_, share)| *share), Some(32));
    }
}
