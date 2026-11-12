//! Open double gates wall off the tiles just past them.
//!
//! Content `general_use/scripts/gates.rs2` (`open_gate`): clicking a
//! closed gate deletes both leaves and re-adds them as straight walls
//! rotated `angle + 3`, the main leaf one tile past the fence and the
//! outer leaf two tiles past it (`gate_set_close(angle, 1|2)`), for 500
//! ticks. Live dossiers (2026-11-12) show exactly that: Falador cow field
//! 1551 (3031,3313) north -> 1552/1556 at (3031,3314)/(3031,3315) west,
//! Lumbridge chicken pen 1551 (3236,3296) east -> (3237,3296)/(3238,3296)
//! north. Plans that walked across those leaves were refused twice
//! (replan = same path) and the scripts walked back through the gate:
//! ~5000 refusals and ~2300 back-and-forth crossings in one day of logs.
//!
//! Every route below is a live route that failed that way. The check
//! replays each walk step on the map as live sees it with the gate open
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

/// One double gate as the pack places it shut: main leaf (category
/// gate_main_closed) and outer leaf (gate_outer_closed).
struct Gate {
    main: (i32, i32),
    outer: (i32, i32),
}

/// Server `gate_set_close(angle, 1)` offset: the side the leaves swing to.
fn swing_offset(angle: u8) -> (i32, i32) {
    match angle {
        0 => (-1, 0),
        1 => (0, 1),
        2 => (1, 0),
        _ => (0, -1),
    }
}

/// The map as live sees it with this gate open, 6 tiles around it.
fn open_gate_view(world: &StaticWorld, gate: &Gate) -> CollisionMap {
    let (main_x, main_z) = gate.main;
    let mut view = CollisionMap::new();
    for x in main_x - 6..=main_x + 6 {
        for z in main_z - 6..=main_z + 6 {
            view.set(x, z, 0, world.get(x, z, 0));
        }
    }
    let (main_id, main_shape, main_angle) = world.door_at(main_x, main_z, 0).expect("main leaf in the door table");
    let (outer_id, outer_shape, outer_angle) =
        world.door_at(gate.outer.0, gate.outer.1, 0).expect("outer leaf in the door table");
    assert_eq!((main_id, main_shape), (1551, 0), "main leaf is gate 1551");
    assert_eq!((outer_id, outer_shape), (1553, 0), "outer leaf is gate 1553");
    world.unpaint_into(&mut view, main_id, main_shape, main_angle, main_x, main_z, 0);
    world.unpaint_into(&mut view, outer_id, outer_shape, outer_angle, gate.outer.0, gate.outer.1, 0);
    let (step_x, step_z) = swing_offset(main_angle);
    let open_angle = (main_angle + 3) % 4;
    for distance in 1..=2 {
        view.change_wall(main_x + step_x * distance, main_z + step_z * distance, 0, open_angle, 0, false, true);
    }
    view
}

/// Every walk step of the plan, replayed on the open-gate view near the
/// gate (static map elsewhere). Door segments move the walker to their
/// cross tile; the crossing itself is the executor's business.
fn leaf_crossings(world: &StaticWorld, view: &CollisionMap, gate: &Gate, from: Tile, plan: &Plan) -> Vec<String> {
    let near = |x: i32, z: i32| (x - gate.main.0).abs() <= 6 && (z - gate.main.1).abs() <= 6;
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
            Segment::OpenDoor { cross_to, .. } => position = *cross_to,
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => position = *expect,
        }
    }
    bad
}

fn check(name: &str, from: Tile, dest: Tile, gate: Gate) {
    let Some((world, tables)) = setup() else { return };
    let options = NavOptions { f2p_only: true, ..NavOptions::default() };
    let plan = plan::plan(&world, &tables, from, dest, &options).unwrap_or_else(|no| panic!("{name}: no route {no:?}"));
    let view = open_gate_view(&world, &gate);
    let bad = leaf_crossings(&world, &view, &gate, from, &plan);
    assert!(bad.is_empty(), "{name}: plan walks across the open gate leaves {bad:?} (cost {})", plan.cost);
}

fn tile(x: i32, z: i32) -> Tile {
    Tile { x, z, level: 0 }
}

/// bdr53jee t=15553: cow field -> north, refused (3031,3314)->(3030,3314).
#[test]
fn falador_cow_field_gate_exit_avoids_leaves() {
    check(
        "falador_cow",
        tile(3034, 3301),
        tile(3015, 3354),
        Gate { main: (3031, 3313), outer: (3032, 3313) },
    );
}

/// b3gybcev t=9875: chicken pen -> Varrock, refused (3237,3296)->(3237,3297).
#[test]
fn lumbridge_chicken_pen_gate_exit_avoids_leaves() {
    check(
        "chicken_out",
        tile(3228, 3296),
        tile(3252, 3419),
        Gate { main: (3236, 3296), outer: (3236, 3295) },
    );
}

/// bcm9kxn4 t=7048: back into the pen from the north, refused
/// (3237,3297)->(3237,3296) on the open leaf 1552.
#[test]
fn lumbridge_chicken_pen_gate_entry_avoids_leaves() {
    check(
        "chicken_in",
        tile(3236, 3302),
        tile(3230, 3298),
        Gate { main: (3236, 3296), outer: (3236, 3295) },
    );
}

/// bmzvbp5u t=5014: farm gate west of Lumbridge, refused (3188,3280)->(3187,3280).
#[test]
fn lumbridge_farm_gate_avoids_leaves() {
    check(
        "farm_gate",
        tile(3184, 3279),
        tile(3183, 3279),
        Gate { main: (3188, 3279), outer: (3189, 3279) },
    );
}

/// bd23dqpq t=15344: east cow field -> Varrock, refused (3251,3266)->(3251,3265).
#[test]
fn lumbridge_cow_field_gate_avoids_leaves() {
    check(
        "cow_gate",
        tile(3260, 3290),
        tile(3256, 3419),
        Gate { main: (3253, 3266), outer: (3253, 3267) },
    );
}

/// bbkjz2vk t=98352: Falador south-west gate, refused (2924,3290)->(2925,3290).
#[test]
fn falador_south_west_gate_avoids_leaves() {
    check(
        "falador_sw",
        tile(2924, 3292),
        tile(2927, 3290),
        Gate { main: (2924, 3292), outer: (2923, 3292) },
    );
}
