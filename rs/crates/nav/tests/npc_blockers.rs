//! Static player-blocking NPCs, recomputed from the server (plan P8).
//!
//! The engine paints `BLOCK_NPC_AND_PLAYERS` under every NPC whose type
//! says `blockwalk=all` (`World.ts` spawn, `PathingEntity` moves); one
//! that also says `moverestrict=nomove` never leaves its spawn, so its
//! tiles are static collision our map pack does not carry. Live
//! 2026-11-17: 1 168 failure dossiers, 31 accounts, at (3138,3351±3) by
//! the Draynor "nasty trees" (npc 152 `nasty_tree`, four spawns), every
//! step onto them refused with `flags=0x0` on our side.
//!
//! `gamedata/data/npc_blockers.json` lists those spawns (type configs in
//! `../lostcity/content/scripts/**/*.npc`, spawns in `npcs.json`); the
//! nav reads it (`Tables::npc_blockers`). Recomputed here; on mismatch
//! `LIFESCAPE_WRITE_DUMPS=1` rewrites this checkout's table.

use std::collections::{BTreeMap, BTreeSet};

use lifescape_json::Value;
use lifescape_nav::collision::tile_walkable;
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gamedata/data")
}

fn npc_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<std::path::PathBuf> = entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            npc_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "npc") {
            out.push(path);
        }
    }
}

/// NPC type names (with their size) the server makes static blockers.
fn blocker_types() -> BTreeMap<String, i64> {
    let mut files = Vec::new();
    npc_files(&workspace_root().join("lostcity/content/scripts"), &mut files);
    assert!(!files.is_empty(), "server npc configs read");
    let mut out = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let mut name: Option<String> = None;
        let (mut block_all, mut no_move, mut size) = (false, false, 1);
        let flush = |name: &Option<String>, block_all: bool, no_move: bool, size: i64, out: &mut BTreeMap<String, i64>| {
            if let (Some(name), true, true) = (name, block_all, no_move) {
                out.entry(name.clone()).or_insert(size);
            }
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix('[') {
                flush(&name, block_all, no_move, size, &mut out);
                name = rest.split(']').next().map(str::to_string);
                (block_all, no_move, size) = (false, false, 1);
            } else if line == "blockwalk=all" {
                block_all = true;
            } else if line == "moverestrict=nomove" {
                no_move = true;
            } else if let Some(value) = line.strip_prefix("size=") {
                size = value.trim().parse().unwrap_or(1);
            }
        }
        flush(&name, block_all, no_move, size, &mut out);
    }
    out
}

fn read_json(name: &str) -> Value {
    let path = data_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    lifescape_json::parse(&text).unwrap_or_else(|err| panic!("{name}: {err}"))
}

#[test]
fn static_npc_blockers_match_the_server() {
    let types = blocker_types();
    assert!(types.contains_key("nasty_tree"), "the Draynor nasty tree blocks players: {types:?}");
    let mut rows: BTreeSet<(i64, i64, i64, i64, String)> = BTreeSet::new();
    for npc in read_json("npcs.json").as_arr().cloned().unwrap_or_default() {
        let Some(name) = npc.get("name").as_str() else { continue };
        let Some(size) = types.get(name) else { continue };
        for spawn in npc.get("spawns").as_arr().cloned().unwrap_or_default() {
            let (Some(x), Some(z), Some(level)) = (spawn.get("x").as_i64(), spawn.get("z").as_i64(), spawn.get("level").as_i64()) else { continue };
            rows.insert((x, z, level, *size, name.to_string()));
        }
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|(x, z, level, size, name)| format!("{{\"x\":{x},\"z\":{z},\"level\":{level},\"size\":{size},\"name\":\"{name}\"}}"))
        .collect();
    let table = format!("{{\"blockers\":[\n{}\n]}}\n", lines.join(",\n"));
    let path = data_dir().join("npc_blockers.json");
    if std::env::var("LIFESCAPE_WRITE_DUMPS").is_ok() {
        std::fs::write(&path, &table).expect("write npc_blockers.json");
    }
    let shipped = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(shipped, table, "npc_blockers.json drifted from the server configs; rerun with LIFESCAPE_WRITE_DUMPS=1");
}

/// Golden: the nasty tree's tile (3138,3352) is no walk any more, the
/// grass beside it still is.
#[test]
fn the_nasty_tree_tile_blocks_the_walk() {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(data_dir().join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data_dir().join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let tables = Tables::load(&data_dir()).expect("tables");
    assert!(tile_walkable(world.get(3138, 3352, 0)), "the pack alone leaves it open");
    world.set_npc_blockers(tables.npc_blockers.clone());
    assert!(!tile_walkable(world.get(3138, 3352, 0)), "the nasty tree stands there");
    assert!(tile_walkable(world.get(3137, 3352, 0)), "the grass beside it");
}
