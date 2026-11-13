//! Furniture that "opens" is never a passage.
//!
//! Drawers, cupboards and chests (categories `drawer` 37,
//! `cupboard_closed` 35, `chest_closed` 33) advertise Open, block walk,
//! and open into a variant that still blocks walk (categories 38 / 36 /
//! 34, all `blockwalk` in the dumps; `general_use/scripts/drawers.rs2`,
//! `cupboards.rs2`, `chests.rs2` only swap the loc). The planner read
//! them as doors and planned "open the drawers, step onto them": live
//! 2026-11-13, melee bots in the Varrock south-east house walked to the
//! drawers 350 at (3236,3405) / (3236,3408) and failed ~60 times in one
//! hour with `DoorDidNotOpen` ("I can't reach that!", crossing refused).

use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn setup() -> Option<(Arc<StaticWorld>, Tables)> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let engine = match std::env::var("LIFESCAPE_ENGINE_DIR") {
        Ok(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
        _ => manifest.join("../../../../lostcity/engine"),
    };
    let zip = engine.join("data/pack/.cache/maps-server.zip");
    if !zip.is_file() {
        return None;
    }
    let data = manifest.join("../../../gamedata/data");
    let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&engine.join("../content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&data).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    Some((world, tables))
}

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

/// Walking to a drawers tile stops beside it: no door segment, a
/// substituted destination next to the drawers.
fn check_stops_beside(name: &str, from: Tile, drawers: Tile) {
    let Some((world, tables)) = setup() else { return };
    assert_eq!(world.loc_at(drawers.x, drawers.z, 0).map(|(id, _, _)| id), Some(350), "{name}: drawers2 in the pack");
    assert!(world.door_at(drawers.x, drawers.z, 0).is_none(), "{name}: drawers are no door");
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let plan = plan::plan(&world, &tables, from, drawers, &options).unwrap_or_else(|no| panic!("{name}: no route {no:?}"));
    let doors: Vec<u16> = plan
        .segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::OpenDoor { door, .. } => Some(door.loc),
            _ => None,
        })
        .collect();
    assert!(!doors.contains(&350), "{name}: plan opens the drawers {doors:?}");
    let adjusted = plan.adjusted_dest.unwrap_or_else(|| panic!("{name}: blocked drawers tile needs a substitute"));
    assert!(adjusted.chebyshev_to(drawers) <= 1, "{name}: substitute {adjusted:?} beside the drawers");
}

trait Near {
    fn chebyshev_to(&self, other: Tile) -> i32;
}

impl Near for Tile {
    fn chebyshev_to(&self, other: Tile) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

/// bh99tw2f t=1498: kill at (3236,3404), walk_to (3236,3405) -> door 350.
#[test]
fn varrock_house_south_drawers_are_no_door() {
    check_stops_beside("south_drawers", tile(3236, 3404), tile(3236, 3405));
}

/// bh99tw2f t=896: kill at (3236,3404), walk_to (3236,3408) -> door 350.
#[test]
fn varrock_house_north_drawers_are_no_door() {
    check_stops_beside("north_drawers", tile(3236, 3404), tile(3236, 3408));
}
