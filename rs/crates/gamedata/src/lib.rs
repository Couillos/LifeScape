//! Game data tables: typed accessors over the generated JSON dumps.
//!
//! Data files live in `gamedata/data/*.json` (arrays indexed by id:
//! the row index *is* the id). F2P planning filters live in `f2p`
//! (zones in `free_zones`), item provenance in `item_sources`, the XP
//! table in `xp`. This crate ships a small std-only JSON reader so the
//! workspace stays dependency-free.

pub mod banks;
pub mod combat_spots;
pub mod danger;
pub mod drops;
pub mod equipment;
pub mod extra_recipes;
pub mod fishing;
pub mod food;
pub mod gear;
pub mod f2p;
pub mod free_zones;
pub mod item_sources;
pub mod obtain;
pub mod methods;
pub mod npc_ranges;
pub mod player_design;
pub mod quest_requirements;
pub mod quests;
pub mod recipes;
pub mod rocks;
pub mod script_sources;
pub mod services;
pub mod shop_access;
pub mod shops;
pub mod spawns;
pub mod spells;
pub mod stations;
pub mod trees;
pub mod varps;
pub mod xp;

// ---------------------------------------------------------------------------
// Generated tables (one module per dump, emitted by build.rs into OUT_DIR)
// ---------------------------------------------------------------------------

include!(concat!(env!("OUT_DIR"), "/comps_generated.rs"));
include!(concat!(env!("OUT_DIR"), "/objs_generated.rs"));
include!(concat!(env!("OUT_DIR"), "/npcs_generated.rs"));
include!(concat!(env!("OUT_DIR"), "/locs_generated.rs"));

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
// Typed rows: objs / npcs / locs (build-time statics, zero retained heap)
// ---------------------------------------------------------------------------

/// Game object (id plus optional debug name).
#[derive(Debug, Clone)]
pub struct Obj {
    pub obj_id: usize,
    pub name: Option<&'static str>,
    pub display: Option<&'static str>,
    pub desc: Option<&'static str>,
    pub ops: &'static [Option<&'static str>],
    pub inv_ops: &'static [Option<&'static str>],
    pub price: i64,
    pub alch_high: i64,
    pub alch_low: i64,
    pub stackable: bool,
    pub members: Option<bool>,
    /// Server `oc_tradeable` flag (banknotes inherit their link). Missing
    /// in older dumps reads as true, the server default.
    pub tradeable: bool,
    pub respawn_ticks: i64,
}

/// All objects, indexed by id (build-time static, no allocation).
pub fn all_objs() -> &'static [Obj] {
    objs::ALL
}

/// Lowercase exact-name ids in one sorted index: `partition_point` brackets
/// every same-name row, so all matches come back in dump order exactly like
/// the old linear scan (no duplicates in today's dumps).
fn ids_by_exact_name(index: &[(&str, usize)], name: &str) -> Vec<usize> {
    let wanted = name.to_lowercase();
    let start = index.partition_point(|entry| entry.0 < wanted.as_str());
    let mut end = start;
    while end < index.len() && index[end].0 == wanted.as_str() {
        end += 1;
    }
    index[start..end].iter().map(|entry| entry.1).collect()
}

/// Object by id, or None when out of range.
pub fn obj_by_id(obj_id: usize) -> Option<&'static Obj> {
    objs::ALL.get(obj_id)
}

/// Display name as the game shows it ("Air rune"), not the debug name.
pub fn obj_display_name(obj_id: usize) -> Option<&'static str> {
    obj_by_id(obj_id).and_then(|entry| entry.display.or(entry.name))
}

/// Exact (case-insensitive) name match.
pub fn objs_by_name(name: &str) -> Vec<&'static Obj> {
    ids_by_exact_name(objs::BY_NAME, name)
        .into_iter()
        .filter_map(|id| objs::ALL.get(id))
        .collect()
}

/// Substring (case-insensitive) name search.
pub fn objs_search(fragment: &str) -> Vec<&'static Obj> {
    let wanted = fragment.to_lowercase();
    objs::ALL
        .iter()
        .filter(|entry| entry.name.is_some_and(|name| name.to_lowercase().contains(wanted.as_str())))
        .collect()
}

/// NPC spawn point (tile plus facing and level).
#[derive(Debug, Clone)]
pub struct NpcSpawn {
    pub x: i64,
    pub z: i64,
    pub level: i64,
    pub members: bool,
}

/// Non-player character (debug name, combat stats, spawn table).
#[derive(Debug, Clone)]
pub struct Npc {
    pub npc_id: usize,
    pub name: Option<&'static str>,
    pub size: i64,
    pub ops: &'static [Option<&'static str>],
    pub members: Option<bool>,
    pub hunts_players: bool,
    pub huntrange: i64,
    pub vislevel: i64,
    pub spawns: &'static [NpcSpawn],
}

/// All NPCs, indexed by id (build-time static, no allocation).
pub fn all_npcs() -> &'static [Npc] {
    npcs::ALL
}

/// NPC by id, or None when out of range.
pub fn npc_by_id(npc_id: usize) -> Option<&'static Npc> {
    npcs::ALL.get(npc_id)
}

/// Exact (case-insensitive) name match.
pub fn npcs_by_name(name: &str) -> Vec<&'static Npc> {
    ids_by_exact_name(npcs::BY_NAME, name)
        .into_iter()
        .filter_map(|id| npcs::ALL.get(id))
        .collect()
}

/// Substring (case-insensitive) name search.
pub fn npcs_search(fragment: &str) -> Vec<&'static Npc> {
    let wanted = fragment.to_lowercase();
    npcs::ALL
        .iter()
        .filter(|entry| entry.name.is_some_and(|name| name.to_lowercase().contains(wanted.as_str())))
        .collect()
}

/// Static scenery (footprint, collision flags, ops, spawns).
#[derive(Debug, Clone)]
pub struct Loc {
    pub loc_id: usize,
    pub name: Option<&'static str>,
    pub width: i64,
    pub length: i64,
    pub blockwalk: bool,
    pub blockrange: bool,
    pub active: i64,
    pub ops: &'static [Option<&'static str>],
    pub spawns: &'static [LocSpawn],
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

/// All scenery, indexed by id (build-time static, no allocation).
pub fn all_locs() -> &'static [Loc] {
    locs::ALL
}

/// Scenery by id, or None when out of range.
pub fn loc_by_id(loc_id: usize) -> Option<&'static Loc> {
    locs::ALL.get(loc_id)
}

/// Exact (case-insensitive) name match.
pub fn locs_by_name(name: &str) -> Vec<&'static Loc> {
    ids_by_exact_name(locs::BY_NAME, name)
        .into_iter()
        .filter_map(|id| locs::ALL.get(id))
        .collect()
}

/// Component id by dotted name (`magic:wind_strike`), like `comps.com_id`.
pub fn com_id(name: &str) -> Option<u16> {
    let wanted = name.to_lowercase();
    comps::BY_NAME
        .binary_search_by(|entry| entry.0.cmp(wanted.as_str()))
        .ok()
        .map(|position| comps::BY_NAME[position].1)
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

    /// The by-name obj lookup answers exactly like the old full scan
    /// (coins is the canonical probe) and stays fast when gear planning
    /// resolves hundreds of names per spot: the whole equipment table
    /// must resolve well inside one server tick budget.
    #[test]
    fn obj_by_name_matches_and_stays_fast() {
        let coins = super::objs_by_name("coins");
        assert_eq!(coins.len(), 1, "one coins row, got {}", coins.len());
        assert_eq!(coins[0].obj_id, 995);
        assert_eq!(super::objs_by_name("COINS")[0].obj_id, 995, "case-insensitive");
        assert!(super::objs_by_name("no_such_obj_xyz").is_empty(), "unknown stays empty");
        let equips = super::equipment::all_wearables();
        assert!(!equips.is_empty(), "wearables load");
        let start = std::time::Instant::now();
        let mut hits = 0usize;
        for entry in &equips {
            if super::objs_by_name(&entry.name).into_iter().next().is_some() {
                hits += 1;
            }
        }
        assert!(hits > 0, "some wearables resolve");
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "678 lookups under 2 s, took {:?}", start.elapsed());
    }

    /// The build embeds the dump: known names resolve to their dump ids,
    /// matching stays case-insensitive and unknown names stay `None`.
    #[test]
    fn com_id_reads_the_build_time_table() {
        assert_eq!(super::com_id("inventory:inv"), Some(3214));
        assert_eq!(super::com_id("music:competition"), Some(10983));
        assert_eq!(super::com_id("INVENTORY:INV"), Some(3214));
        assert_eq!(super::com_id("no_such_comp:xx"), None);
    }

    /// The generated statics hold every dump row at its dump id, with the
    /// same fields the old runtime rows carried (spot-checked against the
    /// dump values asserted by each table module's own tests).
    #[test]
    fn generated_tables_match_the_dumps() {
        assert_eq!(super::all_objs().len(), 3894);
        assert_eq!(super::all_npcs().len(), 1359);
        assert_eq!(super::all_locs().len(), 4671);

        let coins = super::obj_by_id(995).expect("coins in dumps");
        assert_eq!(coins.name, Some("coins"));
        assert_eq!(coins.display, Some("Coins"));
        assert_eq!(coins.price, 1);
        assert_eq!(coins.ops.len(), 5);
        let hans = super::npc_by_id(0).expect("hans in dumps");
        assert_eq!(hans.name, Some("hans"));
        assert_eq!(hans.spawns.len(), 1);
        assert_eq!((hans.spawns[0].x, hans.spawns[0].z, hans.spawns[0].level), (3207, 3233, 0));
        let booth = super::loc_by_id(2213).expect("bank booth in dumps");
        assert_eq!(booth.name, Some("bankbooth"));
        assert!(!booth.spawns.is_empty(), "booth has spawns");

        let npc_spawns: usize = super::all_npcs().iter().map(|entry| entry.spawns.len()).sum();
        assert_eq!(npc_spawns, 7322, "npc spawn count, got {npc_spawns}");
        let loc_spawns: usize = super::all_locs().iter().map(|entry| entry.spawns.len()).sum();
        assert_eq!(loc_spawns, 15030, "loc spawn count, got {loc_spawns}");
    }

    /// Name indexes stay sorted, resolve to their own row, and keep the
    /// dump order the old linear scans returned.
    #[test]
    fn name_indexes_cover_every_row() {
        for (index, rows, label) in [
            (super::objs::BY_NAME, super::all_objs().len(), "objs"),
            (super::npcs::BY_NAME, super::all_npcs().len(), "npcs"),
            (super::locs::BY_NAME, super::all_locs().len(), "locs"),
        ] {
            assert!(index.windows(2).all(|pair| pair[0].0 <= pair[1].0), "{label} index sorted");
            for (name, id) in index {
                assert!(*id < rows, "{label} index id {id} in range");
                assert!(!name.is_empty(), "{label} index name non-empty");
            }
        }
        // Multi-row order matches the scan: substring hits come back in id order.
        let banks: Vec<usize> = super::all_locs()
            .iter()
            .filter(|entry| entry.name.is_some_and(|name| name.contains("bank")))
            .map(|entry| entry.loc_id)
            .collect();
        let mut ordered = banks.clone();
        ordered.sort_unstable();
        assert_eq!(banks, ordered, "substring hits in id order");
        let booths = super::locs_by_name("BANKBOOTH");
        assert_eq!(booths.len(), 1, "one bankbooth row, got {}", booths.len());
        assert_eq!(booths[0].loc_id, 2213);
        assert!(super::npc_by_id(1359).is_none(), "npc out of range stays None");
        assert!(super::loc_by_id(4671).is_none(), "loc out of range stays None");
        assert!(super::obj_by_id(3894).is_none(), "obj out of range stays None");
    }
}
