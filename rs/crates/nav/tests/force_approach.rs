//! Ride stands honour the engine `forceapproach` (rotated by angle).
//!
//! The server reaches such a loc from one side only; any other stand
//! answers "I can't reach that!" and never climbs. Live 2026-11-13
//! (admin, castle top 1740 at (3205,3208,2), angle 3, forceapproach
//! east): west (3204,3208) and east (3206,3208) refused, north
//! (3205,3209) rode to (3205,3209,1).

use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir).join("../LifeScape");
        }
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn setup() -> (Arc<StaticWorld>, Tables) {
    let root = workspace_root();
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, tables)
}

fn stands_of(world: &StaticWorld, tables: &Tables, loc: u16, from: Tile) -> Vec<(i32, i32)> {
    let row = tables
        .transitions
        .iter()
        .find(|row| row.loc == loc && row.from == from)
        .unwrap_or_else(|| panic!("no transition row for loc {loc} at {from:?}"));
    let options = NavOptions { allow_paid: true, f2p_only: true, ..NavOptions::default() };
    plan::ride_stands(world, tables, &options, row).into_iter().map(|stand| (stand.x, stand.z)).collect()
}

#[test]
fn castle_top_stairs_ride_from_their_approach_side_only() {
    let (world, tables) = setup();
    // South tower top: angle 3, east rotates to north.
    assert_eq!(stands_of(&world, &tables, 1740, Tile { x: 3205, z: 3208, level: 2 }), vec![(3205, 3209)]);
    // North tower top: angle 0, east stays east (golden chateau_toit_descente).
    assert_eq!(stands_of(&world, &tables, 1740, Tile { x: 3205, z: 3229, level: 2 }), vec![(3206, 3229)]);
}

#[test]
fn wide_stairs_use_engine_width_and_length() {
    let (world, tables) = setup();
    // Varrock west bank stairs (1722, 2x3, angle 3, forceapproach
    // south rotated to east): footprint x 3156..3158, z 3435..3436, so
    // only the east column answers; (3156,3437) refused live.
    assert_eq!(stands_of(&world, &tables, 1722, Tile { x: 3156, z: 3435, level: 0 }), vec![(3159, 3435), (3159, 3436)]);
}

#[test]
fn transitions_table_carries_force_approach() {
    let (_world, tables) = setup();
    let flagged = tables.transitions.iter().filter(|row| row.force_approach != 0).count();
    assert!(flagged > 0, "no transition row carries force_approach: transitions.json predates the dump field");
    // spiralstairs (1738) has no forceapproach in stairs.loc.
    assert!(tables.transitions.iter().filter(|row| row.loc == 1738).all(|row| row.force_approach == 0));
    // spiralstairstop: forceapproach=east, all sides but east refused.
    assert!(tables.transitions.iter().filter(|row| row.loc == 1740).all(|row| row.force_approach == 0b1101));
}
