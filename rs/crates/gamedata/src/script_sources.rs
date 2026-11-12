//! Script-driven ways to obtain objects, dumped by
//! `gamedata/tools/dump_plan.mts` from the rev 274 sources:
//!
//! - `dialogue_buys.json`: NPCs selling through dialogue (Karim's kebab,
//!   bartenders' beer, Aggie's dyes): coins out, object in.
//! - `script_grants.json`: every `inv_add(inv, obj, n)` outside drop
//!   tables and shops, with the block that runs it (`[opheldu,…]`,
//!   `[oploc1,…]`, `[opnpc1,…]`, a quest proc) and what it consumes.
//! - `config_recipes.json`: enchant spells (`convertobj`), dye mixing and
//!   cape dyeing (structs), leftovers after eating (`next_obj_stage`).
//!
//! Members gating is decided by the callers (`item_sources`): F2P
//! objects, F2P quests, NPCs standing in F2P zones.

#![forbid(unsafe_code)]

use std::sync::OnceLock;

const BUYS: &str = include_str!("../../../../gamedata/data/dialogue_buys.json");
const GRANTS: &str = include_str!("../../../../gamedata/data/script_grants.json");
const CONFIG: &str = include_str!("../../../../gamedata/data/config_recipes.json");

/// One dialogue sale: `cost` coins for `qty` of `obj` from `npc`.
#[derive(Debug, Clone)]
pub struct DialogueBuy {
    pub npc: String,
    pub npc_id: i64,
    pub obj: String,
    pub qty: i64,
    pub cost: i64,
    pub file: String,
}

/// One scripted grant of `obj`.
#[derive(Debug, Clone)]
pub struct Grant {
    pub obj: String,
    pub qty: i64,
    /// Block trigger (`opheldu`, `oploc1`, `opnpc1`, `label`, `proc`…).
    pub trigger: String,
    /// Block subject (the held obj, loc, NPC, label or proc name).
    pub subject: String,
    /// Objects the same block deletes (item-on-item inputs), with counts.
    pub consumes: Vec<(String, i64)>,
    /// Coins the block takes (0 when free).
    pub coins: i64,
    /// Source file relative to `content/scripts`.
    pub file: String,
}

impl Grant {
    /// Quest directory when the grant lives under `quests/quest_x/`.
    pub fn quest_dir(&self) -> Option<&str> {
        let rest = self.file.strip_prefix("quests/")?;
        rest.split('/').next().filter(|dir| dir.starts_with("quest_"))
    }
}

/// One config-driven production.
#[derive(Debug, Clone)]
pub struct ConfigRecipe {
    /// `spell`, `mix`, `dye_cape` or `leftover`.
    pub kind: String,
    pub inputs: Vec<String>,
    pub output: String,
    /// Spellbook key for `spell` rows (`enchant_lvl3`).
    pub spell: Option<String>,
}

fn strings(value: &lifescape_json::Value) -> Vec<String> {
    value.as_arr().cloned().unwrap_or_default().iter().filter_map(|entry| entry.as_str().map(str::to_string)).collect()
}

/// Every dialogue sale.
pub fn dialogue_buys() -> &'static [DialogueBuy] {
    static TABLE: OnceLock<Vec<DialogueBuy>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let Ok(doc) = lifescape_json::parse(BUYS) else { return Vec::new() };
        doc.get("buys")
            .as_arr()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| DialogueBuy {
                npc: row.get("npc").as_str().unwrap_or("").to_string(),
                npc_id: row.get("npc_id").as_i64().unwrap_or(-1),
                obj: row.get("obj").as_str().unwrap_or("").to_string(),
                qty: row.get("qty").as_i64().unwrap_or(1),
                cost: row.get("cost").as_i64().unwrap_or(0),
                file: row.get("file").as_str().unwrap_or("").to_string(),
            })
            .collect()
    })
}

/// Every scripted grant.
pub fn grants() -> &'static [Grant] {
    static TABLE: OnceLock<Vec<Grant>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let Ok(doc) = lifescape_json::parse(GRANTS) else { return Vec::new() };
        doc.get("grants")
            .as_arr()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| Grant {
                obj: row.get("obj").as_str().unwrap_or("").to_string(),
                qty: row.get("qty").as_i64().unwrap_or(1),
                trigger: row.get("trigger").as_str().unwrap_or("").to_string(),
                subject: row.get("subject").as_str().unwrap_or("").to_string(),
                consumes: strings(&row.get("consumes"))
                    .iter()
                    .filter_map(|pair| {
                        let (name, count) = pair.split_once(':')?;
                        Some((name.to_string(), count.parse().unwrap_or(1)))
                    })
                    .collect(),
                coins: row.get("coins").as_i64().unwrap_or(0),
                file: row.get("file").as_str().unwrap_or("").to_string(),
            })
            .collect()
    })
}

/// Player-facing handlers (`opnpc1,duke_horacio`, `oplocu,crystal_chest`,
/// `opheldu,bowl_water`) of one granting file.
pub fn file_anchors(file: &str) -> Vec<(String, String)> {
    static TABLE: OnceLock<std::collections::HashMap<String, Vec<(String, String)>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut out = std::collections::HashMap::new();
        let Ok(doc) = lifescape_json::parse(GRANTS) else { return out };
        if let Some(rows) = doc.get("anchors").as_obj() {
            for (file, list) in rows {
                let pairs = strings(list)
                    .iter()
                    .filter_map(|pair| pair.split_once(',').map(|(trigger, subject)| (trigger.to_string(), subject.to_string())))
                    .collect();
                out.insert(file.clone(), pairs);
            }
        }
        out
    });
    table.get(file).cloned().unwrap_or_default()
}

/// Quest directories whose varps a granting or selling file reads: a
/// coarse hint (a file may read a quest only for dialogue), used where a
/// scripted object is most likely a quest reward (gear choice).
pub fn file_quests(file: &str) -> Vec<String> {
    static TABLE: OnceLock<std::collections::HashMap<String, Vec<String>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut out = std::collections::HashMap::new();
        let Ok(doc) = lifescape_json::parse(GRANTS) else { return out };
        if let Some(rows) = doc.get("file_quests").as_obj() {
            for (file, list) in rows {
                out.insert(file.clone(), strings(list));
            }
        }
        out
    });
    table.get(file).cloned().unwrap_or_default()
}

/// Every config-driven production.
pub fn config_recipes() -> &'static [ConfigRecipe] {
    static TABLE: OnceLock<Vec<ConfigRecipe>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let Ok(doc) = lifescape_json::parse(CONFIG) else { return Vec::new() };
        doc.get("recipes")
            .as_arr()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| ConfigRecipe {
                kind: row.get("kind").as_str().unwrap_or("").to_string(),
                inputs: strings(&row.get("inputs")),
                output: row.get("output").as_str().unwrap_or("").to_string(),
                spell: row.get("spell").as_str().map(str::to_string),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{config_recipes, dialogue_buys, grants};

    #[test]
    fn script_sources_load_server_truth() {
        let kebab = dialogue_buys().iter().find(|row| row.obj == "kebab").expect("Karim sells kebabs");
        assert_eq!(kebab.cost, 1, "kebab_seller.rs2: 1 coin");
        let garlic = grants().iter().find(|row| row.obj == "garlic").expect("garlic cupboard");
        assert!(garlic.file.contains("cupboards"), "garlic from a cupboard, got {garlic:?}");
        let accuracy = grants().iter().find(|row| row.obj == "amulet_of_accuracy").expect("imp catcher reward");
        assert_eq!(accuracy.quest_dir(), Some("quest_imp"));
        let strength = config_recipes().iter().find(|row| row.output == "amulet_of_strength").expect("ruby enchant");
        assert_eq!(strength.spell.as_deref(), Some("enchant_lvl3"));
        assert_eq!(strength.inputs, vec!["strung_ruby_amulet".to_string()]);
        assert!(config_recipes().iter().any(|row| row.kind == "leftover" && row.inputs[0] == "meat_pie" && row.output == "half_a_meat_pie"));
        assert!(config_recipes().iter().any(|row| row.kind == "mix" && row.output == "greendye"));
    }
}
