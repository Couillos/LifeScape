//! Open castle double doors wall off the tiles their leaves swing onto.
//!
//! Content `doors/scripts/doubledoors.rs2` (`open_double_doors_left` /
//! `_right`): clicking a closed leaf of a `door_left_closed` /
//! `door_right_closed` pair (categories 21 / 23) deletes both leaves and
//! re-adds them for 500 ticks one tile towards `door_open(angle)`, as
//! straight walls: the left leaf rotated `angle + 3`, the right leaf
//! `angle + 1`. Lumbridge castle front door 1516 (3217,3218) / 1519
//! (3217,3219) angle west -> 1517 (3216,3218) south / 1520 (3216,3219)
//! north; the hall door 1516 (3213,3220) / 1519 (3213,3221) -> 1517
//! (3212,3220) / 1520 (3212,3221). Plans that stepped across those open
//! leaves were refused live (2026-11-13: ~730 `StepRefused` in one hour,
//! 12 accounts, the replan walking the same leg). The planner marks the
//! leaf edges (`DOOR_LEAF_*`, cost, never a wall, as single doors).
//!
//! Every route below is a live route that failed that way. The check
//! replays each walk step on the map as live sees it with the doors open
//! (closed leaves removed, open leaves painted): no step may cross a leaf.

use std::sync::Arc;

use lifescape_nav::bfs::can_step;
use lifescape_nav::collision::CollisionMap;
use lifescape_nav::plan::{self, NavOptions, Plan, Segment, Tile};
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

/// Lumbridge castle front door (left leaf 1516, right leaf 1519).
const FRONT_DOOR: [(i32, i32); 2] = [(3217, 3218), (3217, 3219)];
/// Lumbridge castle hall door, west of the front door.
const HALL_DOOR: [(i32, i32); 2] = [(3213, 3220), (3213, 3221)];
const DOORS: [[(i32, i32); 2]; 2] = [FRONT_DOOR, HALL_DOOR];
const RADIUS: i32 = 8;

/// Server `door_open(angle, wall_straight)`: the side the leaves swing to.
fn swing_offset(angle: u8) -> (i32, i32) {
    match angle {
        0 => (-1, 0),
        1 => (0, 1),
        2 => (1, 0),
        _ => (0, -1),
    }
}

fn near(x: i32, z: i32) -> bool {
    DOORS
        .iter()
        .flatten()
        .any(|(door_x, door_z)| (x - door_x).abs() <= RADIUS && (z - door_z).abs() <= RADIUS)
}

/// The map as live sees it with both castle doors open.
fn open_doors_view(world: &StaticWorld) -> CollisionMap {
    let mut view = CollisionMap::new();
    for x in 3213 - RADIUS - 1..=3217 + RADIUS + 1 {
        for z in 3218 - RADIUS - 1..=3221 + RADIUS + 1 {
            view.set(x, z, 0, world.get(x, z, 0));
        }
    }
    for door in DOORS {
        for (leaf_x, leaf_z) in door {
            let (loc_id, shape, angle) = world.door_at(leaf_x, leaf_z, 0).expect("leaf in the door table");
            assert!(matches!(loc_id, 1516 | 1519) && shape == 0, "castle leaf ({leaf_x},{leaf_z}) is {loc_id}/{shape}");
            world.unpaint_into(&mut view, loc_id, shape, angle, leaf_x, leaf_z, 0);
            let (step_x, step_z) = swing_offset(angle);
            let open_angle = if loc_id == 1516 { (angle + 3) % 4 } else { (angle + 1) % 4 };
            view.change_wall(leaf_x + step_x, leaf_z + step_z, 0, open_angle, 0, false, true);
        }
    }
    view
}

/// Every walk step of the plan, replayed on the open-door view near the
/// doors (static map elsewhere). A door segment walks to its stand tile
/// first, then moves the walker to its cross tile.
fn leaf_crossings(world: &StaticWorld, view: &CollisionMap, from: Tile, plan: &Plan) -> Vec<String> {
    let mut flags = |x: i32, z: i32| if near(x, z) { view.get(x, z, 0) } else { world.get(x, z, 0) };
    let mut position = from;
    let mut bad = Vec::new();
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => {
                for tile in tiles {
                    if (tile.x, tile.z) == (position.x, position.z) {
                        continue;
                    }
                    if tile.level == 0
                        && near(tile.x, tile.z)
                        && !can_step(&mut flags, position.x, position.z, tile.x, tile.z)
                    {
                        bad.push(format!("({},{})->({},{})", position.x, position.z, tile.x, tile.z));
                    }
                    position = *tile;
                }
            }
            Segment::OpenDoor { stand, cross_to, .. } => {
                if (stand.x, stand.z) != (position.x, position.z)
                    && near(stand.x, stand.z)
                    && !can_step(&mut flags, position.x, position.z, stand.x, stand.z)
                {
                    bad.push(format!("({},{})->stand ({},{})", position.x, position.z, stand.x, stand.z));
                }
                position = *cross_to;
            }
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => position = *expect,
        }
    }
    bad
}

fn check(name: &str, from: Tile, dest: Tile) {
    let Some((world, tables)) = setup() else { return };
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let plan = plan::plan(&world, &tables, from, dest, &options).unwrap_or_else(|no| panic!("{name}: no route {no:?}"));
    let view = open_doors_view(&world);
    let bad = leaf_crossings(&world, &view, from, &plan);
    assert!(bad.is_empty(), "{name}: plan walks across the open door leaves {bad:?} (cost {})", plan.cost);
}

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

/// leo_king t=8185: castle hall -> Market, refused (3216,3220)->(3216,3219)
/// on the open right leaf 1520.
#[test]
fn castle_front_door_to_market_avoids_leaves() {
    check("front_market", tile(3216, 3220), tile(3097, 3238));
}

/// Melee bots, castle hall -> men east of the castle, same refusal.
#[test]
fn castle_front_door_to_men_avoids_leaves() {
    check("front_men", tile(3216, 3220), tile(3236, 3210));
}

/// Castle south wing -> north courtyard, refused (3216,3217)->(3216,3218)
/// on the open left leaf 1517.
#[test]
fn castle_front_door_from_south_avoids_leaves() {
    check("front_south", tile(3216, 3217), tile(3207, 3229));
}

/// West hall -> men, refused (3212,3222)->(3212,3221) on the open hall
/// door leaf 1520.
#[test]
fn castle_hall_door_avoids_leaves() {
    check("hall_men", tile(3212, 3222), tile(3236, 3210));
}

/// Entering the castle (open since the one-way overrides went): from the
/// courtyard to the hall north of the front door, never across a leaf.
#[test]
fn castle_front_door_entry_avoids_leaves() {
    check("front_entry", tile(3222, 3218), tile(3215, 3221));
}
