//! Companion of `dump-npc-ranges` (plan rendements R2, priors): the
//! success odds the server rolls for gathering and cooking, read from
//! the server configs (read-only) and written to
//! `gamedata/data/action_odds.json`:
//! - `rocks`: ore product -> `rock_successchance` (low, high),
//!   `skill_mining/configs/mine.dbrow`;
//! - `rock_respawn`: ore product -> `rock_respawnrate` (ticks a mined
//!   rock stays empty, `mining.rs2` `loc_change`); a rock without one
//!   (rune essence) never depletes;
//! - `pickaxes`: pickaxe -> `mining_rate` (ticks between swings minus
//!   one), `skill_mining/configs/pickaxes.obj`;
//! - `trees`: log product -> axe -> `successchance` (low, high),
//!   `skill_woodcutting/configs/trees.dbrow`;
//! - `tree_respawn`: log product -> `respawnrate` of the same table
//!   (0: a normal tree, felled by every log and back after 120 +
//!   random(80) ticks; else felled by 1 log in 8, `woodcutting.rs2`);
//! - `fish`: raw fish -> `success_low`, `success_high` of its
//!   `fishing_struct`, `skill_fishing/configs/fishing.obj|.struct`;
//! - `cooking`: uncooked -> cooked, burnt, `successchance` (fire) and
//!   `successchance_range`, `skill_cooking/configs/cooking_source/
//!   cooking_generic.dbrow`;
//! - `runecraft`: rune -> `multiplier` (runes per essence =
//!   floor(level / multiplier) + 1, `runecraft.rs2`),
//!   `skill_runecraft/configs/runecraft.dbrow`;
//! - `mining_gems`: the gem roll of a mined rock (`mining.rs2`
//!   `get_ore_normal`: one roll in `random($chance)`, the F2P chance,
//!   the members amulet aside) and `mining_gem_table` (`random(draws)`,
//!   each gem's share = its threshold minus the previous one), the
//!   source of uncut gems.
//!
//! Every pair feeds `stat_random(stat, low, high)` (engine
//! `PlayerOps.ts`): the read side (`gamedata::action_odds`) turns it into
//! odds at a level.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::Path;

use lifescape_json::{object, str_value, Value};

/// One `[block]` of a config file: its `key=value` lines in order.
pub type Block = Vec<(String, String)>;

/// Splits a config text into `[name]` blocks (comments dropped).
pub fn blocks(text: &str) -> Vec<(String, Block)> {
    let mut out: Vec<(String, Block)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            out.push((name.to_string(), Vec::new()));
            continue;
        }
        if let (Some((key, value)), Some(last)) = (line.split_once('='), out.last_mut()) {
            last.1.push((key.to_string(), value.to_string()));
        }
    }
    out
}

/// Values of every `data=<column>,...` line of a dbrow block.
fn data(block: &Block, column: &str) -> Vec<Vec<String>> {
    block
        .iter()
        .filter(|(key, _)| key == "data")
        .filter_map(|(_, value)| {
            let mut parts = value.split(',').map(|part| part.trim().to_string());
            (parts.next()? == column).then(|| parts.collect())
        })
        .collect()
}

/// Value of `param=<name>,<value>` in an obj/struct block.
fn param(block: &Block, name: &str) -> Option<String> {
    block.iter().filter(|(key, _)| key == "param").find_map(|(_, value)| value.split_once(',').filter(|(key, _)| *key == name).map(|(_, rest)| rest.trim().to_string()))
}

fn pair(values: &[String]) -> Option<(i64, i64)> {
    Some((values.first()?.parse().ok()?, values.get(1)?.parse().ok()?))
}

fn pair_value(low_high: (i64, i64)) -> Value {
    Value::Arr(vec![Value::Int(low_high.0), Value::Int(low_high.1)])
}

/// Text of the `[header]` section of a script (up to the next one).
fn section<'a>(script: &'a str, header: &str) -> Option<&'a str> {
    let start = script.find(header)? + header.len();
    let rest = &script[start..];
    Some(rest.find("\n[").map_or(rest, |end| &rest[..end]))
}

/// Integer between `prefix` and `suffix` on the first line holding both.
fn number_between(text: &str, prefix: &str, suffix: &str) -> Option<i64> {
    text.lines().find_map(|line| line.trim().strip_prefix(prefix)?.strip_suffix(suffix)?.trim().parse().ok())
}

/// The gem roll of a mined rock and its table (`mining.rs2`), `None`
/// when the script's shape moved (never a guessed table): `roll` from
/// `def_int $chance = N;` of `get_ore_normal` (rolled by
/// `random($chance) = ^true`), `draws` from the proc's `random(N)`, each
/// gem's share from the cumulative `if ($rand < X) return (gem);`.
pub fn mining_gems_value(script: &str) -> Option<Value> {
    let normal = section(script, "[label,get_ore_normal]")?;
    if !normal.contains("random($chance) = ^true") {
        return None;
    }
    let roll = number_between(normal, "def_int $chance =", ";")?;
    let table = section(script, "[proc,mining_gem_table]")?;
    let draws = number_between(table, "def_int $rand = random(", ");")?;
    let mut gems = BTreeMap::new();
    let mut previous = 0;
    for line in table.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("if ($rand < ") else { continue };
        let (threshold, gem) = rest.split_once(") return (")?;
        let threshold: i64 = threshold.trim().parse().ok()?;
        let gem = gem.strip_suffix(");")?.trim();
        if threshold <= previous || threshold > draws {
            return None;
        }
        gems.insert(gem.to_string(), Value::Int(threshold - previous));
        previous = threshold;
    }
    (roll > 0 && !gems.is_empty()).then(|| object(vec![("roll", Value::Int(roll)), ("draws", Value::Int(draws)), ("gems", Value::Obj(gems))]))
}

/// The odds document from the config texts and the mining script.
#[allow(clippy::too_many_arguments)]
pub fn odds_value(mine: &str, pickaxes: &str, trees: &str, fish_objs: &str, fish_structs: &str, cooking: &str, runecraft: &str, mining_script: &str) -> Value {
    let mut rocks = BTreeMap::new();
    let mut rock_respawn = BTreeMap::new();
    for (_, block) in blocks(mine) {
        let output = data(&block, "rock_output").first().and_then(|values| values.first().cloned());
        let respawn = data(&block, "rock_respawnrate").first().and_then(|values| values.first()).and_then(|value| value.parse::<i64>().ok());
        if let (Some(output), Some(chance)) = (output, data(&block, "rock_successchance").first().and_then(|values| pair(values))) {
            // The same table as the odds (the last row of an output wins).
            match respawn {
                Some(respawn) => rock_respawn.insert(output.clone(), Value::Int(respawn)),
                None => rock_respawn.remove(&output),
            };
            rocks.insert(output, pair_value(chance));
        }
    }
    let mut picks = BTreeMap::new();
    for (name, block) in blocks(pickaxes) {
        if let Some(rate) = param(&block, "mining_rate").and_then(|value| value.parse::<i64>().ok()) {
            picks.insert(name, Value::Int(rate));
        }
    }
    let mut logs = BTreeMap::new();
    let mut tree_respawn = BTreeMap::new();
    for (_, block) in blocks(trees) {
        let Some(product) = data(&block, "product").first().and_then(|values| values.first().cloned()) else { continue };
        let mut axes = BTreeMap::new();
        for row in data(&block, "successchance") {
            if let (Some(axe), Some(chance)) = (row.first(), pair(&row[1.min(row.len())..])) {
                axes.insert(axe.clone(), pair_value(chance));
            }
        }
        // Several tables yield plain logs (dead trees...): the first
        // block with odds wins, odds-less blocks add nothing.
        if !axes.is_empty() && !logs.contains_key(&product) {
            if let Some(respawn) = data(&block, "respawnrate").first().and_then(|values| values.first()).and_then(|value| value.parse::<i64>().ok()) {
                tree_respawn.insert(product.clone(), Value::Int(respawn));
            }
            logs.insert(product, Value::Obj(axes));
        }
    }
    let structs: BTreeMap<String, Block> = blocks(fish_structs).into_iter().collect();
    let mut fish = BTreeMap::new();
    for (name, block) in blocks(fish_objs) {
        let Some(found) = param(&block, "fishing_struct").and_then(|key| structs.get(&key)) else { continue };
        let low = param(found, "success_low").and_then(|value| value.parse::<i64>().ok());
        let high = param(found, "success_high").and_then(|value| value.parse::<i64>().ok());
        if let (Some(low), Some(high)) = (low, high) {
            fish.insert(name, pair_value((low, high)));
        }
    }
    let mut cooks = BTreeMap::new();
    for (_, block) in blocks(cooking) {
        let first = |column: &str| data(&block, column).first().and_then(|values| values.first().cloned());
        let (Some(uncooked), Some(cooked)) = (first("uncooked"), first("cooked")) else { continue };
        let Some(fire) = data(&block, "successchance").first().and_then(|values| pair(values)) else { continue };
        let mut fields = vec![("cooked", str_value(&cooked)), ("fire", pair_value(fire))];
        if let Some(burnt) = first("burnt") {
            fields.push(("burnt", str_value(&burnt)));
        }
        if let Some(range) = data(&block, "successchance_range").first().and_then(|values| pair(values)) {
            fields.push(("range", pair_value(range)));
        }
        cooks.insert(uncooked, object(fields));
    }
    let mut runes = BTreeMap::new();
    for (_, block) in blocks(runecraft) {
        let first = |column: &str| data(&block, column).first().and_then(|values| values.first().cloned());
        if let (Some(rune), Some(multiplier)) = (first("rune"), first("multiplier").and_then(|value| value.parse::<i64>().ok())) {
            runes.insert(rune, Value::Int(multiplier));
        }
    }
    object(vec![
        ("source", str_value("lostcity/content/scripts skill_mining, skill_woodcutting, skill_fishing, skill_cooking, skill_runecraft configs, mining.rs2 (orchestrator dump-npc-ranges)")),
        ("mining_gems", mining_gems_value(mining_script).unwrap_or(Value::Null)),
        ("rocks", Value::Obj(rocks)),
        ("rock_respawn", Value::Obj(rock_respawn)),
        ("pickaxes", Value::Obj(picks)),
        ("trees", Value::Obj(logs)),
        ("tree_respawn", Value::Obj(tree_respawn)),
        ("fish", Value::Obj(fish)),
        ("cooking", Value::Obj(cooks)),
        ("runecraft", Value::Obj(runes)),
    ])
}

/// Reads the server configs under `scripts` and writes the odds file.
pub fn dump_action_odds(scripts: &Path, output: &Path) -> Result<usize, String> {
    let read = |relative: &str| std::fs::read_to_string(scripts.join(relative)).map_err(|error| format!("{relative}: {error}"));
    let doc = odds_value(
        &read("skill_mining/configs/mine.dbrow")?,
        &read("skill_mining/configs/pickaxes.obj")?,
        &read("skill_woodcutting/configs/trees.dbrow")?,
        &read("skill_fishing/configs/fishing.obj")?,
        &read("skill_fishing/configs/fishing.struct")?,
        &read("skill_cooking/configs/cooking_source/cooking_generic.dbrow")?,
        &read("skill_runecraft/configs/runecraft.dbrow")?,
        &read("skill_mining/scripts/mining.rs2")?,
    );
    if doc.get("mining_gems").as_obj().is_none() {
        return Err("skill_mining/scripts/mining.rs2: gem roll or mining_gem_table moved".to_string());
    }
    let count = ["rocks", "rock_respawn", "pickaxes", "trees", "tree_respawn", "fish", "cooking", "runecraft"].iter().map(|key| doc.get(key).as_obj().map(|map| map.len()).unwrap_or(0)).sum::<usize>() + doc.get("mining_gems").get("gems").as_obj().map(|map| map.len()).unwrap_or(0);
    std::fs::write(output, lifescape_json::stringify_pretty(&doc) + "\n").map_err(|error| format!("{}: {error}", output.display()))?;
    println!("dump-action-odds: {count} rows -> {}", output.display());
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::odds_value;

    #[test]
    fn reads_every_odds_table() {
        let mine = "[copper_rock_table]\ntable=mining_table\ndata=rock,copperrock1\ndata=rock_output,copper_ore\ndata=rock_successchance,128,400\ndata=rock_respawnrate,10\n[rune_essence_table]\ndata=rock_output,blankrune\ndata=rock_successchance,256,256\n";
        let picks = "[bronze_pickaxe]\nname=Bronze pickaxe\nparam=mining_rate,7\n";
        let trees = "[normal_tree_table]\ndata=product,logs\ndata=successchance,bronze_axe,64,200\ndata=successchance,rune_axe,224,700\ndata=respawnrate,0\n[yew_tree_table]\ndata=product,yew_logs\ndata=successchance,bronze_axe,4,12\ndata=respawnrate,200\n";
        let fish_objs = "[raw_shrimp]\nparam=fishing_struct,fishing_struct_shrimps\n";
        let fish_structs = "[fishing_struct_shrimps]\nparam=productexp,100\nparam=success_low,48\nparam=success_high,256\n";
        let cooking = "//c\n[cooking_generic_shrimp]\ntable=cooking_generic\ndata=uncooked,raw_shrimp\ndata=cooked,shrimp\ndata=burnt,burnt_shrimp\ndata=successchance,128,512\n";
        let runecraft = "[runecraft_air]\ntable=runecraft_table\ndata=rune,airrune\ndata=multiplier,11\n";
        let doc = odds_value(mine, picks, trees, fish_objs, fish_structs, cooking, runecraft, MINING_SCRIPT);
        assert_eq!(doc.get("runecraft").get("airrune").as_i64(), Some(11));
        assert_eq!(doc.get("rocks").get("copper_ore").as_arr().and_then(|pair| pair.get(1)).and_then(|value| value.as_i64()), Some(400));
        assert_eq!(doc.get("pickaxes").get("bronze_pickaxe").as_i64(), Some(7));
        assert_eq!(doc.get("rock_respawn").get("copper_ore").as_i64(), Some(10), "a mined rock stays empty");
        assert!(doc.get("rock_respawn").get("blankrune").as_i64().is_none(), "essence never depletes");
        assert_eq!(doc.get("tree_respawn").get("logs").as_i64(), Some(0), "a normal tree: felled by every log");
        assert_eq!(doc.get("tree_respawn").get("yew_logs").as_i64(), Some(200));
        assert_eq!(doc.get("trees").get("logs").get("rune_axe").as_arr().and_then(|pair| pair.first()).and_then(|value| value.as_i64()), Some(224));
        assert_eq!(doc.get("fish").get("raw_shrimp").as_arr().and_then(|pair| pair.first()).and_then(|value| value.as_i64()), Some(48));
        assert_eq!(doc.get("cooking").get("raw_shrimp").get("burnt").as_str(), Some("burnt_shrimp"));
        assert!(doc.get("cooking").get("raw_shrimp").get("range").as_arr().is_none());
        let gems = doc.get("mining_gems");
        assert_eq!(gems.get("roll").as_i64(), Some(256), "one roll in random($chance)");
        assert_eq!(gems.get("draws").as_i64(), Some(128));
        assert_eq!(gems.get("gems").get("uncut_diamond").as_i64(), Some(2));
        assert_eq!(gems.get("gems").get("uncut_ruby").as_i64(), Some(8), "10 - 2");
        assert_eq!(gems.get("gems").get("uncut_sapphire").as_i64(), Some(32), "58 - 26");
    }

    /// The server's shape (`mining.rs2`): the gem roll of a normal rock,
    /// the members amulet aside, then the table proc (thresholds
    /// cumulative over `random(128)`).
    const MINING_SCRIPT: &str = "[label,get_ore_normal]\ndb_find(mining_table:rock, loc_type);\n// roll for gem\ndef_int $chance = 256;\ndef_obj $neck = inv_getobj(worn, ^wearpos_front);\nif ($neck ! null & oc_category($neck) = category_557 & map_members = ^true) {\n    $chance = 86;\n}\nif (random($chance) = ^true) {\n    def_namedobj $gem = ~mining_gem_table;\n}\n\n[proc,mining_gem_table]()(namedobj)\ndef_int $rand = random(128);\nif ($rand < 2) return (uncut_diamond);\nif ($rand < 10) return (uncut_ruby);\nif ($rand < 26) return (uncut_emerald);\nif ($rand < 58) return (uncut_sapphire);\nelse return(null);\n";

    /// A script whose gem roll or table moved yields no table (never a
    /// guessed one).
    #[test]
    fn a_moved_gem_table_is_not_guessed() {
        assert!(super::mining_gems_value("[label,get_ore_normal]\nif (random(256) = ^true) {}\n").is_none());
        assert!(super::mining_gems_value(&MINING_SCRIPT.replace("def_int $rand = random(128);", "")).is_none());
    }

    /// Writes `gamedata/data/action_odds.json` from the server configs
    /// (run by hand: `cargo test --release -p lifescape-orchestrator
    /// write_action_odds -- --ignored`), like `dump-npc-ranges` does.
    #[test]
    #[ignore]
    fn write_action_odds() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scripts = std::env::var("LIFESCAPE_CONTENT_SCRIPTS").map(std::path::PathBuf::from).unwrap_or_else(|_| root.join("../lostcity/content/scripts"));
        let count = super::dump_action_odds(&scripts, &root.join("gamedata/data/action_odds.json")).expect("dump");
        assert!(count > 40, "{count}");
    }
}
