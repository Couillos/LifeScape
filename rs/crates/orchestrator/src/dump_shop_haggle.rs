//! Companion of `dump-npc-ranges` (decision utilisateur 2026-11-16 §2,
//! finite shop stock): the `shop_delta` ("haggle") each shop prices its
//! stock with, read from the server configs (read-only) and written to
//! `gamedata/data/shop_haggle.json`:
//! - `default`: the param's default (`shop/configs/shopkeeper.param`
//!   `[shop_delta]`), for a keeper whose config sets none;
//! - `keepers`: NPC debugname -> `param=shop_delta,N` of its `.npc`
//!   block (`~openshop_activenpc` reads it);
//! - `opens`: shop inventory -> the haggle argument of a scripted
//!   `~openshop(inv, buy, sell, haggle, title)` call (Zaff, Rommik...),
//!   the largest when several calls open it.
//!
//! The server prices every unit bought at `calc_shop_value(cost,
//! haggle, shop_sell_multiplier, stock - base)` (`shop.rs2`
//! `adjusted_item_cost_buying`): the read side (`gamedata::shop_buy`)
//! walks the stock down unit by unit.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lifescape_json::{object, str_value, Value};

/// `[shop_delta]` default of `shopkeeper.param`.
pub fn param_default(param_text: &str) -> Option<i64> {
    let block = crate::dump_action_odds::blocks(param_text).into_iter().find(|(name, _)| name == "shop_delta")?;
    block.1.iter().find(|(key, _)| key == "default").and_then(|(_, value)| value.trim().parse().ok())
}

/// NPC debugname -> `shop_delta` of every `.npc` block that sets it.
pub fn keeper_deltas(npc_texts: &[String]) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for text in npc_texts {
        for (name, block) in crate::dump_action_odds::blocks(text) {
            let delta = block.iter().filter(|(key, _)| key == "param").find_map(|(_, value)| value.strip_prefix("shop_delta,").and_then(|rest| rest.trim().parse::<i64>().ok()));
            if let Some(delta) = delta {
                out.insert(name, delta);
            }
        }
    }
    out
}

/// Shop inventory -> haggle argument of the scripted `~openshop(...)`
/// calls (the largest when several open the same inventory).
pub fn open_deltas(script_texts: &[String]) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for text in script_texts {
        for call in text.split("~openshop(").skip(1) {
            let Some(arguments) = call.split(')').next() else { continue };
            let parts: Vec<&str> = arguments.split(',').map(str::trim).collect();
            let (Some(inv), Some(haggle)) = (parts.first(), parts.get(3).and_then(|value| value.parse::<i64>().ok())) else { continue };
            if inv.starts_with('$') || inv.is_empty() {
                continue;
            }
            let known = out.entry(inv.to_string()).or_insert(haggle);
            *known = (*known).max(haggle);
        }
    }
    out
}

/// The haggle document.
pub fn haggle_value(param_text: &str, npc_texts: &[String], script_texts: &[String]) -> Result<Value, String> {
    let default = param_default(param_text).ok_or_else(|| "shopkeeper.param: no [shop_delta] default".to_string())?;
    let keepers = keeper_deltas(npc_texts).into_iter().map(|(npc, delta)| (npc, Value::Int(delta))).collect();
    let opens = open_deltas(script_texts).into_iter().map(|(inv, delta)| (inv, Value::Int(delta))).collect();
    Ok(object(vec![
        ("source", str_value("lostcity/content/scripts shop/configs/shopkeeper.param, **/*.npc shop_delta, **/*.rs2 ~openshop (orchestrator dump-npc-ranges)")),
        ("default", Value::Int(default)),
        ("keepers", Value::Obj(keepers)),
        ("opens", Value::Obj(opens)),
    ]))
}

fn files_with(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            files_with(&path, extension, out)?;
        } else if path.extension().is_some_and(|ext| ext == extension) {
            out.push(path);
        }
    }
    Ok(())
}

fn read_all(scripts: &Path, extension: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    files_with(scripts, extension, &mut files)?;
    files.sort();
    files.iter().map(|path| std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))).collect()
}

/// Reads the server scripts under `scripts` and writes the haggle file.
pub fn dump_shop_haggle(scripts: &Path, output: &Path) -> Result<usize, String> {
    let param = std::fs::read_to_string(scripts.join("shop/configs/shopkeeper.param")).map_err(|error| format!("shopkeeper.param: {error}"))?;
    let doc = haggle_value(&param, &read_all(scripts, "npc")?, &read_all(scripts, "rs2")?)?;
    let count = doc.get("keepers").as_obj().map(|map| map.len()).unwrap_or(0) + doc.get("opens").as_obj().map(|map| map.len()).unwrap_or(0);
    std::fs::write(output, lifescape_json::stringify_pretty(&doc) + "\n").map_err(|error| format!("{}: {error}", output.display()))?;
    println!("dump-shop-haggle: {count} rows -> {}", output.display());
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::haggle_value;

    /// The server's shapes: the param default, a keeper's `.npc` block,
    /// a scripted opener (Rommik), an opener reading a variable (never
    /// an inventory).
    #[test]
    fn reads_defaults_keepers_and_scripted_openers() {
        let param = "[shop_sell_multiplier]\ntype=int\ndefault=100\n\n[shop_delta]\ntype=int\ndefault=10\n";
        let npcs = vec!["[gerrant]\nname=Gerrant\ncategory=shop_keeper\nparam=owned_shop,fishingshop\nparam=shop_sell_multiplier,1000\nparam=shop_delta,30\n[man]\nname=Man\n".to_string()];
        let scripts = vec!["[opnpc3,rommik]\nif (map_members = ^true) {\n    ~openshop(craftingshop, 650, 1000, 20, \"Rommiks Crafty Supplies.\");\n}\n[proc,openshop](inv $shop, int $shop_buy, int $shop_sell, int $shop_haggle, string $title)\n~openshop($shop, 1, 2, 3, \"x\");\n".to_string()];
        let doc = haggle_value(param, &npcs, &scripts).expect("doc");
        assert_eq!(doc.get("default").as_i64(), Some(10));
        assert_eq!(doc.get("keepers").get("gerrant").as_i64(), Some(30));
        assert!(doc.get("keepers").get("man").as_i64().is_none());
        assert_eq!(doc.get("opens").get("craftingshop").as_i64(), Some(20));
        assert_eq!(doc.get("opens").as_obj().map(|map| map.len()), Some(1), "a variable inventory is no shop");
        assert!(haggle_value("[shop_title]\ntype=string\n", &npcs, &scripts).is_err(), "no default: no guess");
    }

    /// Writes `gamedata/data/shop_haggle.json` from the server scripts
    /// (run by hand: `cargo test --release -p lifescape-orchestrator
    /// write_shop_haggle -- --ignored`), like `dump-npc-ranges` does.
    #[test]
    #[ignore]
    fn write_shop_haggle() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scripts = std::env::var("LIFESCAPE_CONTENT_SCRIPTS").map(std::path::PathBuf::from).unwrap_or_else(|_| root.join("../lostcity/content/scripts"));
        let count = super::dump_shop_haggle(&scripts, &root.join("gamedata/data/shop_haggle.json")).expect("dump");
        assert!(count > 100, "{count}");
    }
}
