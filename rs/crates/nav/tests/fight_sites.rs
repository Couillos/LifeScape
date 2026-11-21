//! Fight sites of the combat camps, recomputed from the map.
//!
//! `gamedata/data/unreachable_spawns.json` lists every F2P spawn of a
//! combat camp (`combat_spots::catalogue_spots`) no stand reaches in the
//! open world (`lifescape_nav::site_reach::spawn_verdicts`: bars, walls,
//! a sealed room). `combat_spots::positions` leaves them out, so the
//! brain's site table and the ladder's clusters never aim at one (live
//! 2026-11-21: the caged lesser demon at the top of the Wizards' Tower
//! (3110,3157,2), `Disconnected` on every walk, 25 failures in 6 h).
//! Recomputed here; on mismatch the message prints the table,
//! `LIFESCAPE_WRITE_SITES=1` writes it.

use std::sync::Arc;

use lifescape_nav::plan::Tile;
use lifescape_nav::site_reach::{spawn_verdicts, SiteVerdict};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// This checkout's own game data (a worktree reads and writes its own).
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

/// `(x, z, level, spot)` of every F2P spawn of every catalogue camp.
fn camp_spawns() -> Vec<(i64, i64, i64, &'static str)> {
    let mut out: Vec<(i64, i64, i64, &'static str)> = Vec::new();
    for spot in lifescape_gamedata::combat_spots::catalogue_spots() {
        for (x, z, level) in lifescape_gamedata::combat_spots::raw_positions(spot.key) {
            if !out.iter().any(|known| (known.0, known.1, known.2) == (x, z, level)) {
                out.push((x, z, level, spot.key));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn unreachable_fight_sites_match_the_map() {
    let (world, tables) = setup();
    let spawns = camp_spawns();
    assert!(spawns.len() > 200, "F2P camp spawns read: {}", spawns.len());
    let tiles: Vec<Tile> = spawns.iter().map(|(x, z, level, _)| Tile { x: *x as i32, z: *z as i32, level: *level as i32 }).collect();
    let verdicts = spawn_verdicts(&world, &tables, &tiles).expect("edges");
    let mut computed: Vec<String> = Vec::new();
    for ((x, z, level, spot), verdict) in spawns.iter().zip(&verdicts) {
        if matches!(verdict, SiteVerdict::NoStand | SiteVerdict::Sealed) {
            computed.push(format!("{{\"x\":{x},\"z\":{z},\"level\":{level},\"spot\":\"{spot}\",\"reason\":\"{}\"}}", verdict.reason()));
        }
    }
    // The caged demon is out; the volcano's demons stay (boat and rope).
    assert!(computed.iter().any(|line| line.starts_with("{\"x\":3110,\"z\":3157,\"level\":2,")), "the caged demon (3110,3157,2) is unreachable: {computed:?}");
    let volcano = spawns.iter().zip(&verdicts).filter(|((x, z, level, spot), _)| *spot == "lesser_demons" && *level == 0 && (2820..=2850).contains(x) && (9548..=9570).contains(z));
    for ((x, z, _, _), verdict) in volcano {
        assert_eq!(*verdict, SiteVerdict::Reachable, "volcano demon ({x},{z})");
    }
    let table = format!("{{\"sites\":[\n{}\n]}}\n", computed.join(",\n"));
    let path = data_dir().join("unreachable_spawns.json");
    if std::env::var("LIFESCAPE_WRITE_SITES").is_ok() {
        std::fs::write(&path, &table).expect("write unreachable_spawns.json");
    }
    let shipped = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(shipped, table, "unreachable_spawns.json drifted from the map; rerun with LIFESCAPE_WRITE_SITES=1 ({} of {} spawns unreachable)", computed.len(), spawns.len());
}
