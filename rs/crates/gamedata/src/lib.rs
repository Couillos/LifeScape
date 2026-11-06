//! Game data tables: typed accessors over the generated JSON dumps.
//!
//! Data files live in `gamedata/data/*.json` (arrays indexed by id:
//! the row index *is* the id). This crate ships a small
//! std-only JSON reader so the workspace stays dependency-free.

use std::collections::HashMap;
use std::sync::OnceLock;

pub mod banks;
pub mod combat_spots;
pub mod equipment;
pub mod fishing;
pub mod food;
pub mod quest_requirements;
pub mod quests;
pub mod recipes;
pub mod rocks;
pub mod shops;
pub mod spawns;
pub mod spells;
pub mod trees;
pub mod varps;

// ---------------------------------------------------------------------------
// Mini JSON reader (std-only)
// ---------------------------------------------------------------------------

/// JSON rows share the workspace value model: objects keep sorted keys
/// and a missing key reads as null. Parsing lives in `lifescape-json`.
pub use lifescape_json::Value as Json;

#[derive(Debug, Clone)]
pub struct JsonError(pub String);

impl std::fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "json error: {}", self.0)
    }
}

impl std::error::Error for JsonError {}


// ---------------------------------------------------------------------------
// Shared row helpers (std-only, no allocation beyond the result)
// ---------------------------------------------------------------------------

/// Optional string field, owned.
pub(crate) fn opt_str(row: &Json, key: &str) -> Option<String> {
    row.get(key).as_str().map(str::to_string)
}

/// Integer field with a default when missing.
pub(crate) fn num(row: &Json, key: &str, default: i64) -> i64 {
    row.get(key).as_i64().unwrap_or(default)
}

/// Nullable integer field.
pub(crate) fn opt_num(row: &Json, key: &str) -> Option<i64> {
    row.get(key).as_i64()
}

/// Boolean field with a default when missing.
pub(crate) fn flag(row: &Json, key: &str, default: bool) -> bool {
    row.get(key).as_bool().unwrap_or(default)
}

/// Nullable boolean field (single `members` flag, never invented).
pub(crate) fn opt_flag(row: &Json, key: &str) -> Option<bool> {
    row.get(key).as_bool()
}

/// List-of-strings field, skipping nulls.
pub(crate) fn str_list(row: &Json, key: &str) -> Vec<String> {
    match row.get(key) {
        Json::Arr(items) => items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// String->int map field (runes, bonuses, requirements, inputs...).
pub(crate) fn str_int_map(row: &Json, key: &str) -> Vec<(String, i64)> {
    match row.get(key) {
        Json::Obj(fields) => fields
            .iter()
            .filter_map(|(name, value)| value.as_i64().map(|number| (name.clone(), number)))
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Table loading
// ---------------------------------------------------------------------------

fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_GAMEDATA") {
        return std::path::PathBuf::from(dir);
    }
    // Walk up from the binary and from the working directory: the first
    // gamedata/data holding objs.json wins (workspace, crate or install).
    let mut starts = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.push(exe);
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    for start in starts {
        let mut dir = start.as_path();
        for _ in 0..10 {
            let candidate = dir.join("gamedata").join("data");
            if candidate.join("objs.json").exists() {
                return candidate;
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
    }
    // Last resort: the historical relative layout.
    std::path::PathBuf::from("gamedata/data")
}

fn load_value(name: &str) -> Result<Json, JsonError> {
    let path = data_dir().join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).map_err(|err| JsonError(format!("cannot read {}: {err}", path.display())))?;
    lifescape_json::parse(&text).map_err(|err| JsonError(format!("cannot parse {}: {err}", path.display())))
}

/// Every dump table, parsed once and shared. Arrays and objects alike:
/// use `table` for id-indexed arrays, match on the value for dicts.
pub fn table_value(name: &str) -> Option<&'static Json> {
    static CACHE: OnceLock<HashMap<String, Json>> = OnceLock::new();
    const KNOWN: &[&str] = &[
        "objs", "npcs", "locs", "comps", "varps", "shops", "equipment", "foods", "recipes", "rocks", "spells",
        "trees", "quests", "spawns_obj",
    ];
    CACHE
        .get_or_init(|| {
            let mut maps = HashMap::new();
            for known in KNOWN {
                if let Ok(value) = load_value(known) {
                    maps.insert((*known).to_string(), value);
                }
            }
            maps
        })
        .get(name)
}

/// Load (and cache) one array table by name. Rows are indexed by id;
/// unknown tables and non-arrays yield an empty slice.
pub fn table(name: &str) -> &'static [Json] {
    match table_value(name) {
        Some(Json::Arr(rows)) => rows,
        _ => &[],
    }
}

fn row_name(row: &Json) -> Option<String> {
    row.get("name").as_str().map(str::to_string)
}

fn row_ops(row: &Json, key: &str) -> Vec<Option<String>> {
    match row.get(key) {
        Json::Arr(items) => items
            .iter()
            .map(|item| match item {
                Json::Null => None,
                Json::Str(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Typed rows: objs / npcs / locs
// ---------------------------------------------------------------------------

/// Game object (id plus optional debug name).
#[derive(Debug, Clone)]
pub struct Obj {
    pub obj_id: usize,
    pub name: Option<String>,
    pub display: Option<String>,
    pub desc: Option<String>,
    pub ops: Vec<Option<String>>,
    pub inv_ops: Vec<Option<String>>,
    pub price: i64,
    pub alch_high: i64,
    pub alch_low: i64,
    pub stackable: bool,
    pub members: Option<bool>,
    pub respawn_ticks: i64,
}

fn obj_from_row(obj_id: usize, row: &Json) -> Obj {
    if row.is_null() {
        return Obj {
            obj_id,
            name: None,
            display: None,
            desc: None,
            ops: Vec::new(),
            inv_ops: Vec::new(),
            price: 0,
            alch_high: 0,
            alch_low: 0,
            stackable: false,
            members: None,
            respawn_ticks: 100,
        };
    }
    Obj {
        obj_id,
        name: row_name(row),
        display: row.get("display").as_str().map(str::to_string).or_else(|| row_name(row)),
        desc: row.get("desc").as_str().map(str::to_string),
        ops: row_ops(row, "ops"),
        inv_ops: row_ops(row, "iops"),
        price: row.get("price").as_i64().unwrap_or(0),
        alch_high: row.get("alch_high").as_i64().unwrap_or(0),
        alch_low: row.get("alch_low").as_i64().unwrap_or(0),
        stackable: row.get("stackable").as_bool().unwrap_or(false),
        members: row.get("members").as_bool(),
        respawn_ticks: row.get("respawn_ticks").as_i64().unwrap_or(100),
    }
}

/// All objects, indexed by id.
pub fn all_objs() -> Vec<Obj> {
    table("objs").iter().enumerate().map(|(id, row)| obj_from_row(id, row)).collect()
}

/// Object by id, or None when out of range.
pub fn obj_by_id(obj_id: usize) -> Option<Obj> {
    table("objs").get(obj_id).map(|row| obj_from_row(obj_id, row))
}

/// Display name as the game shows it ("Air rune"), not the debug name.
pub fn obj_display_name(obj_id: usize) -> Option<String> {
    obj_by_id(obj_id).and_then(|entry| entry.display.or(entry.name))
}

/// Exact (case-insensitive) name match.
pub fn objs_by_name(name: &str) -> Vec<Obj> {
    let wanted = name.to_lowercase();
    all_objs().into_iter().filter(|entry| entry.name.as_deref().unwrap_or("").to_lowercase() == wanted).collect()
}

/// Substring (case-insensitive) name search.
pub fn objs_search(fragment: &str) -> Vec<Obj> {
    let wanted = fragment.to_lowercase();
    all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").to_lowercase().contains(&wanted))
        .collect()
}

/// NPC spawn point (tile plus facing and level).
#[derive(Debug, Clone)]
pub struct NpcSpawn {
    pub x: i64,
    pub z: i64,
    pub level: i64,
}

/// Non-player character (debug name, combat stats, spawn table).
#[derive(Debug, Clone)]
pub struct Npc {
    pub npc_id: usize,
    pub name: Option<String>,
    pub size: i64,
    pub ops: Vec<Option<String>>,
    pub members: Option<bool>,
    pub hunts_players: bool,
    pub huntrange: i64,
    pub vislevel: i64,
    pub spawns: Vec<NpcSpawn>,
}

fn npc_from_row(npc_id: usize, row: &Json) -> Npc {
    if row.is_null() {
        return Npc { npc_id, name: None, size: 1, ops: Vec::new(), members: None, hunts_players: false, huntrange: 0, vislevel: 0, spawns: Vec::new() };
    }
    let mut spawns = Vec::new();
    if let Json::Arr(items) = row.get("spawns") {
        for item in items {
            if let (Some(x), Some(z), Some(level)) =
                (item.get("x").as_i64(), item.get("z").as_i64(), item.get("level").as_i64())
            {
                spawns.push(NpcSpawn { x, z, level });
            }
        }
    }
    Npc {
        npc_id,
        name: row_name(row),
        size: row.get("size").as_i64().unwrap_or(1),
        ops: row_ops(row, "ops"),
        members: row.get("members").as_bool(),
        hunts_players: row.get("hunts_players").as_bool().unwrap_or(false),
        huntrange: row.get("huntrange").as_i64().unwrap_or(0),
        vislevel: row.get("vislevel").as_i64().unwrap_or(0),
        spawns,
    }
}

/// All NPCs, indexed by id.
pub fn all_npcs() -> Vec<Npc> {
    table("npcs").iter().enumerate().map(|(id, row)| npc_from_row(id, row)).collect()
}

/// NPC by id, or None when out of range.
pub fn npc_by_id(npc_id: usize) -> Option<Npc> {
    table("npcs").get(npc_id).map(|row| npc_from_row(npc_id, row))
}

/// Exact (case-insensitive) name match.
pub fn npcs_by_name(name: &str) -> Vec<Npc> {
    let wanted = name.to_lowercase();
    all_npcs().into_iter().filter(|entry| entry.name.as_deref().unwrap_or("").to_lowercase() == wanted).collect()
}

/// Substring (case-insensitive) name search.
pub fn npcs_search(fragment: &str) -> Vec<Npc> {
    let wanted = fragment.to_lowercase();
    all_npcs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").to_lowercase().contains(&wanted))
        .collect()
}

/// Static scenery (footprint, collision flags, ops, spawns).
#[derive(Debug, Clone)]
pub struct Loc {
    pub loc_id: usize,
    pub name: Option<String>,
    pub width: i64,
    pub length: i64,
    pub blockwalk: bool,
    pub blockrange: bool,
    pub active: i64,
    pub ops: Vec<Option<String>>,
    pub spawns: Vec<LocSpawn>,
}

/// One static scenery spawn (tile, replacement and respawn timer).
/// `members` stays null when the dump says nothing (locs carry no
/// server flag; banks derive it from the position instead).
#[derive(Debug, Clone)]
pub struct LocSpawn {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub members: Option<bool>,
}

fn loc_from_row(loc_id: usize, row: &Json) -> Loc {
    if row.is_null() {
        return Loc {
            loc_id,
            name: None,
            width: 1,
            length: 1,
            blockwalk: false,
            blockrange: false,
            active: 0,
            ops: Vec::new(),
            spawns: Vec::new(),
        };
    }
    let mut spawns = Vec::new();
    if let Json::Arr(items) = row.get("spawns") {
        for item in items {
            if let (Some(x), Some(z), Some(level)) =
                (item.get("x").as_i64(), item.get("z").as_i64(), item.get("level").as_i64())
            {
                spawns.push(LocSpawn { x, z, level, members: item.get("members").as_bool() });
            }
        }
    }
    Loc {
        loc_id,
        name: row_name(row),
        width: row.get("width").as_i64().unwrap_or(1),
        length: row.get("length").as_i64().unwrap_or(1),
        blockwalk: row.get("blockwalk").as_bool().unwrap_or(false),
        blockrange: row.get("blockrange").as_bool().unwrap_or(false),
        active: row.get("active").as_i64().unwrap_or(0),
        ops: row_ops(row, "ops"),
        spawns,
    }
}

/// All scenery, indexed by id.
pub fn all_locs() -> Vec<Loc> {
    table("locs").iter().enumerate().map(|(id, row)| loc_from_row(id, row)).collect()
}

/// Scenery by id, or None when out of range.
pub fn loc_by_id(loc_id: usize) -> Option<Loc> {
    table("locs").get(loc_id).map(|row| loc_from_row(loc_id, row))
}

/// Exact (case-insensitive) name match.
pub fn locs_by_name(name: &str) -> Vec<Loc> {
    let wanted = name.to_lowercase();
    all_locs().into_iter().filter(|entry| entry.name.as_deref().unwrap_or("").to_lowercase() == wanted).collect()
}

/// Component id by dotted name (`magic:wind_strike`), like `comps.com_id`.
pub fn com_id(name: &str) -> Option<u16> {
    let wanted = name.to_lowercase();
    for row in table("comps") {
        if row.get("name").as_str().is_some_and(|label| label.to_lowercase() == wanted) {
            return row.get("id").as_i64().map(|id| id as u16);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn hunter_flags_parsed() {
        let spider = super::npcs_by_name("giantspider1").into_iter().next().expect("spider in dumps");
        assert!(spider.hunts_players, "spiders hunt");
        assert!(spider.huntrange >= 1, "spider range, got {}", spider.huntrange);
        assert_eq!(spider.vislevel, 2, "spider level");
        let bob = super::npcs_by_name("bob").into_iter().next().expect("bob in dumps");
        assert!(!bob.hunts_players, "shopkeepers do not hunt");
    }

    #[test]
    fn parses_mixed_values() {
        let value = lifescape_json::parse(r#"{"a": [1, -2, true, null, "x\"y", 1.5], "b": {}}"#).expect("parses");
        match value {
            super::Json::Obj(fields) => assert_eq!(fields.len(), 2),
            _ => panic!("expected object"),
        }
    }
}
