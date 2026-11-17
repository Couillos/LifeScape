//! Custom edges stand and land on floors, and every landing has a way
//! out (tour 2, P2-2, decision D5).
//!
//! Live 2026-11-17 11:10: bellarhodes desynced on the Karamja boat and
//! reappeared on the Port Sarim deck (3032,3214,1). The gangplank stand
//! (3032,3217,1), also the boat's landing, is a floor on the server
//! (m47_50 `1 24 17: o5`, no `f1`) but no wall or loc paints it, so the
//! static world read it as sky: the deck was a component without exit
//! (2 231 NoRoute in an hour). Sweep over every custom edge: its stand
//! and its landing are walkable, and a plan leaves the landing for the
//! open world (Lumbridge), coins in the pack for the fares.

use std::sync::Arc;

use lifescape_nav::collision::tile_walkable;
use lifescape_nav::plan::{self, NavOptions, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

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

#[test]
fn every_custom_edge_stands_and_lands_on_a_floor() {
    let (world, tables) = setup();
    assert!(!tables.customs.is_empty(), "custom edges loaded");
    let mut bad = Vec::new();
    for edge in &tables.customs {
        for (role, tile) in [("stand", edge.stand), ("to", edge.to)] {
            if !tile_walkable(world.get(tile.x, tile.z, tile.level)) {
                bad.push(format!("{} {role} ({},{},{})", edge.name, tile.x, tile.z, tile.level));
            }
        }
    }
    assert!(bad.is_empty(), "edge tiles read as blocked or sky: {bad:?}");
}

#[test]
fn every_custom_landing_has_a_way_out() {
    let (world, tables) = setup();
    let lumbridge = Tile { x: 3222, z: 3218, level: 0 };
    let mut options = NavOptions { allow_paid: true, ..NavOptions::default() };
    for tag in ["coins:30", "coins:60"] {
        options.satisfied.insert(tag.to_string());
    }
    let mut stuck = Vec::new();
    for edge in &tables.customs {
        if let Err(no) = plan::plan(&world, &tables, edge.to, lumbridge, &options) {
            stuck.push(format!("{} lands ({},{},{}): {}", edge.name, edge.to.x, edge.to.z, edge.to.level, no.reason.as_str()));
        }
    }
    assert!(stuck.is_empty(), "landings without a way out: {stuck:?}");
}
