//! Work sites of the catalogue, recomputed from the map (plan P6).
//!
//! `gamedata/data/unreachable_sites.json` lists every F2P tree and rock
//! spawn no worker can work: no stand reaches the loc (wall, fence,
//! invisible blocker) or the stands that do are sealed pockets
//! (`lifescape_nav::site_reach`). `gamedata::danger::sites` leaves them
//! out of the site catalogue, so the brain never assigns one (live
//! 2026-11-17: 288 NoRoute dossiers on the river willow (3168,3350), 387
//! on trees and rocks behind fences). Recomputed here; on mismatch the
//! message prints the table, `LIFESCAPE_WRITE_SITES=1` writes it.

use std::collections::BTreeSet;
use std::sync::Arc;

use lifescape_json::Value;
use lifescape_nav::plan::Tile;
use lifescape_nav::site_reach::{site_verdicts, SiteVerdict};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// This checkout's own game data (a worktree reads and writes its own,
/// never the main checkout's through the sibling link).
fn data_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gamedata/data")
}

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(data_dir().join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data_dir().join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&data_dir()).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, tables)
}

fn read_json(name: &str) -> Value {
    let path = data_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    lifescape_json::parse(&text).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// Loc names of every tree and rock tier (`trees.json`, `rocks.json`).
fn work_loc_names() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for (file, key) in [("trees.json", "trees"), ("rocks.json", "rocks")] {
        for tier in read_json(file).as_arr().cloned().unwrap_or_default() {
            for name in tier.get(key).as_arr().cloned().unwrap_or_default() {
                if let Some(name) = name.as_str() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

/// `(x, z, level, loc id, name)` of every F2P spawn of a work loc.
fn work_spawns(world: &StaticWorld) -> Vec<(i32, i32, i32, u16, String)> {
    let names = work_loc_names();
    let mut out = Vec::new();
    for (loc_id, loc) in read_json("locs.json").as_arr().cloned().unwrap_or_default().iter().enumerate() {
        let Some(name) = loc.get("name").as_str() else { continue };
        if !names.contains(name) {
            continue;
        }
        for spawn in loc.get("spawns").as_arr().cloned().unwrap_or_default() {
            if spawn.get("members").as_bool() == Some(true) {
                continue;
            }
            let (x, z, level) = (spawn.get("x").as_i64().unwrap_or(0) as i32, spawn.get("z").as_i64().unwrap_or(0) as i32, spawn.get("level").as_i64().unwrap_or(0) as i32);
            if !world.in_f2p(x, z) {
                continue;
            }
            out.push((x, z, level, loc_id as u16, name.to_string()));
        }
    }
    out.sort();
    out
}

fn row(x: i32, z: i32, level: i32, loc: u16, name: &str, reason: &str) -> String {
    format!("{{\"x\":{x},\"z\":{z},\"level\":{level},\"loc\":{loc},\"name\":\"{name}\",\"reason\":\"{reason}\"}}")
}

#[test]
fn unreachable_work_sites_match_the_map() {
    let (world, tables) = setup();
    let mut computed: Vec<String> = Vec::new();
    let spawns = work_spawns(&world);
    assert!(spawns.len() > 500, "F2P trees and rocks read: {}", spawns.len());
    let sites: Vec<(u16, Tile)> = spawns.iter().map(|(x, z, level, loc, _)| (*loc, Tile { x: *x, z: *z, level: *level })).collect();
    let verdicts = site_verdicts(&world, &tables, &sites).expect("edges");
    for ((x, z, level, loc, name), verdict) in spawns.iter().zip(verdicts) {
        if matches!(verdict, SiteVerdict::NoStand | SiteVerdict::Sealed) {
            computed.push(row(*x, *z, *level, *loc, name, verdict.reason()));
        }
    }
    // The live NoRoute willow is out of the catalogue.
    assert!(computed.iter().any(|line| line.starts_with("{\"x\":3168,\"z\":3350,")), "the river willow (3168,3350) is unreachable");
    let table = format!("{{\"sites\":[\n{}\n]}}\n", computed.join(",\n"));
    let path = data_dir().join("unreachable_sites.json");
    if std::env::var("LIFESCAPE_WRITE_SITES").is_ok() {
        std::fs::write(&path, &table).expect("write unreachable_sites.json");
    }
    let shipped = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(shipped, table, "unreachable_sites.json drifted from the map; rerun with LIFESCAPE_WRITE_SITES=1 ({} of {} spawns unreachable)", computed.len(), spawns.len());
}
