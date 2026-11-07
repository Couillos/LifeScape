//! Routes en or: chaque ligne de `routes.txt` planifiee et verifiee.
//!
//! Verdicts, couts et temps par plan affiches (chaque plan < 300 ms en
//! release). Les coordonnees viennent du pack, de transitions.json ou
//! des scripts contenu, jamais inventees.

use std::collections::HashSet;
use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Plan, Segment, Tile};
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

fn parse_tile(raw: &str) -> Tile {
    let nums: Vec<i32> = raw.split(',').map(|part| part.trim().parse().expect("tile numbers")).collect();
    assert_eq!(nums.len(), 3, "tile needs x,z,level, got {raw:?}");
    Tile { x: nums[0], z: nums[1], level: nums[2] }
}

fn segment_levels(plan: &Plan) -> Vec<i32> {
    let mut levels = Vec::new();
    for seg in &plan.segments {
        let level = match seg {
            Segment::Walk { tiles } => tiles.first().map(|tile| tile.level),
            Segment::OpenDoor { door, .. } => Some(door.level),
            Segment::Ride { expect, .. } => Some(expect.level),
            Segment::Custom { expect, .. } => Some(expect.level),
        };
        if let Some(level) = level {
            if levels.last() != Some(&level) {
                levels.push(level);
            }
        }
    }
    levels
}

fn uses_loc(plan: &Plan, tables: &Tables, id: u16) -> bool {
    for seg in &plan.segments {
        match seg {
            Segment::OpenDoor { door, .. } => {
                if door.loc == id {
                    return true;
                }
            }
            Segment::Ride { transition, .. } => {
                if transition.loc == id || transition.climb_loc == id {
                    return true;
                }
            }
            Segment::Custom { edge, .. } => {
                if tables.customs.iter().any(|custom| custom.name == edge.name && custom.loc == id) {
                    return true;
                }
            }
            Segment::Walk { .. } => {}
        }
    }
    false
}

fn has_stand(plan: &Plan, x: i32, z: i32) -> bool {
    for seg in &plan.segments {
        let stand = match seg {
            Segment::Walk { .. } => None,
            Segment::OpenDoor { stand, .. } => Some(*stand),
            Segment::Ride { stand, .. } => Some(*stand),
            Segment::Custom { stand, .. } => Some(*stand),
        };
        if stand.is_some_and(|tile| tile.x == x && tile.z == z) {
            return true;
        }
    }
    false
}

fn touches(plan: &Plan, x: i32, z: i32) -> bool {
    for seg in &plan.segments {
        match seg {
            Segment::Walk { tiles } => {
                if tiles.iter().any(|tile| tile.x == x && tile.z == z) {
                    return true;
                }
            }
            Segment::OpenDoor { stand, cross_to, .. } => {
                if (stand.x, stand.z) == (x, z) || (cross_to.x, cross_to.z) == (x, z) {
                    return true;
                }
            }
            Segment::Ride { stand, expect, .. } | Segment::Custom { stand, expect, .. } => {
                if (stand.x, stand.z) == (x, z) || (expect.x, expect.z) == (x, z) {
                    return true;
                }
            }
        }
    }
    false
}

fn check_route(world: &Arc<StaticWorld>, tables: &Tables, line: &str) {    let cols: Vec<&str> = line.split('|').map(str::trim).collect();
    assert_eq!(cols.len(), 5, "route needs 5 | columns, got {line:?}");
    let (name, from, dest, options_raw, assertions_raw) = (cols[0], parse_tile(cols[1]), parse_tile(cols[2]), cols[3], cols[4]);
    let mut options = NavOptions::default();
    if options_raw != "-" {
        for token in options_raw.split(|char| char == ',' || char == ' ' || char == '\t').filter(|token| !token.is_empty()) {
            if token == "paid" {
                options.allow_paid = true;
            } else if token == "f2p" {
                options.f2p_only = true;
            } else if let Some(tag) = token.strip_prefix("has=") {
                options.satisfied.insert(tag.to_string());
            } else {
                panic!("route {name}: unknown option {token:?}");
            }
        }
    }
    let start = std::time::Instant::now();
    let result = plan::plan(world, tables, from, dest, &options);
    let ms = start.elapsed().as_millis();
    let summary = match &result {
        Ok(plan) => {
            let (mut rides, mut doors, mut walk_tiles) = (0, 0, 0);
            for seg in &plan.segments {
                match seg {
                    Segment::Walk { tiles } => walk_tiles += tiles.len(),
                    Segment::OpenDoor { .. } => doors += 1,
                    Segment::Ride { .. } => rides += 1,
                    Segment::Custom { .. } => doors += 1,
                }
            }
            let mut text = format!("cost={} rides={rides} doors={doors} walk={walk_tiles}", plan.cost);
            if let Some(adjusted) = plan.adjusted_dest {
                text += &format!(" adjusted=({},{},{})", adjusted.x, adjusted.z, adjusted.level);
            }
            text
        }
        Err(no) => format!(
            "{} closest=({},{},{})",
            no.reason.as_str(),
            no.closest_reachable.x,
            no.closest_reachable.z,
            no.closest_reachable.level,
        ),
    };
    println!("{name}: {summary} [{ms}ms]");
    // The latency budget guards plans (live travel must stay snappy);
    // a NoRoute-by-exhaustion is bounded by MAX_VISITS and the plan
    // wall-clock budget, not by finding a route, and its milliseconds
    // stay printed above for the record.
    if result.is_ok() && !cfg!(debug_assertions) {
        assert!(ms < 300, "route {name}: plan took {ms}ms, budget is 300ms");
    }
    for assertion in assertions_raw.split(';').map(str::trim).filter(|part| !part.is_empty()) {
        if assertion == "reachable" {
            assert!(result.is_ok(), "route {name}: expected a plan, got {:?}", result.as_ref().err().map(|no| no.reason));
        } else if let Some(reason) = assertion.strip_prefix("unreachable=") {
            let no = result.as_ref().err().unwrap_or_else(|| panic!("route {name}: expected NoRoute({reason}), got a plan"));
            assert_eq!(no.reason.as_str(), reason, "route {name}: wrong NoRoute reason");
        } else if let Some(id) = assertion.strip_prefix("uses_loc=") {
            let id: u16 = id.parse().expect("uses_loc id");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan for uses_loc"));
            assert!(uses_loc(plan, tables, id), "route {name}: no segment uses loc {id}");
        } else if let Some(bound) = assertion.strip_prefix("rides<=") {
            let bound: usize = bound.parse().expect("rides bound");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let count = plan.segments.iter().filter(|seg| matches!(seg, Segment::Ride { .. })).count();
            assert!(count <= bound, "route {name}: {count} rides > {bound}");
        } else if let Some(bound) = assertion.strip_prefix("rides>=") {
            let bound: usize = bound.parse().expect("rides bound");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let count = plan.segments.iter().filter(|seg| matches!(seg, Segment::Ride { .. })).count();
            assert!(count >= bound, "route {name}: {count} rides < {bound}");
        } else if let Some(bound) = assertion.strip_prefix("doors<=") {
            let bound: usize = bound.parse().expect("doors bound");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let count = plan
                .segments
                .iter()
                .filter(|seg| matches!(seg, Segment::OpenDoor { .. } | Segment::Custom { .. }))
                .count();
            assert!(count <= bound, "route {name}: {count} doors > {bound}");
        } else if let Some(bound) = assertion.strip_prefix("walk_tiles<=") {
            let bound: usize = bound.parse().expect("walk bound");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let count: usize = plan
                .segments
                .iter()
                .map(|seg| match seg {
                    Segment::Walk { tiles } => tiles.len(),
                    _ => 0,
                })
                .sum();
            assert!(count <= bound, "route {name}: {count} walk tiles > {bound}");
        } else if let Some(bound) = assertion.strip_prefix("walk_tiles>=") {
            let bound: usize = bound.parse().expect("walk bound");
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let count: usize = plan
                .segments
                .iter()
                .map(|seg| match seg {
                    Segment::Walk { tiles } => tiles.len(),
                    _ => 0,
                })
                .sum();
            assert!(count >= bound, "route {name}: {count} walk tiles < {bound}");
        } else if let Some(levels) = assertion.strip_prefix("levels=") {
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let seen: Vec<String> = segment_levels(plan).iter().map(|level| level.to_string()).collect();
            assert_eq!(seen.join(">"), levels, "route {name}: wrong level trail");
        } else if assertion == "adjusted_dest" {
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            assert!(plan.adjusted_dest.is_some(), "route {name}: expected an adjusted destination");
        } else if let Some(stand) = assertion.strip_prefix("stand=") {
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let pair: Vec<i32> = stand.split(',').map(|part| part.trim().parse().expect("stand pair")).collect();
            assert!(pair.len() == 2, "route {name}: stand needs x,z");
            assert!(has_stand(plan, pair[0], pair[1]), "route {name}: no segment stands on ({},{})", pair[0], pair[1]);
        } else if let Some(avoid) = assertion.strip_prefix("avoids=") {
            let plan = result.as_ref().unwrap_or_else(|_| panic!("route {name}: expected a plan"));
            let pair: Vec<i32> = avoid.split(',').map(|part| part.trim().parse().expect("avoids pair")).collect();
            assert!(pair.len() == 2, "route {name}: avoids needs x,z");
            assert!(!touches(plan, pair[0], pair[1]), "route {name}: plan touches ({},{})", pair[0], pair[1]);
        } else {
            panic!("route {name}: unknown assertion {assertion:?}");
        }
    }
    // Every door stand must be a tile the engine would take the click
    // from: orthogonal neighbour passing the server reach rule for the
    // door's shape and angle. A diagonal or wrong-sided stand reads
    // "I can't reach that!" live (castle double doors 1516/1519).
    if let Ok(plan) = &result {
        for seg in &plan.segments {
            if let Segment::OpenDoor { door, stand, cross_to } = seg {
                let manhattan = (stand.x - door.x).abs() + (stand.z - door.z).abs();
                assert_eq!(manhattan, 1, "route {name}: door {} stand ({},{}) is not orthogonal", door.loc, stand.x, stand.z);
                let (_, shape, angle) = world
                    .door_at(door.x, door.z, door.level)
                    .unwrap_or_else(|| panic!("route {name}: door {} has no static shape", door.loc));
                let (width, height) = world
                    .loc_def(door.loc as usize)
                    .map(|def| (def.length, def.width))
                    .unwrap_or((1, 1));
                let flag = world.get(stand.x, stand.z, stand.level);
                assert!(
                    lifescape_nav::reach::reached(
                        &|_, _, _| flag,
                        stand.level,
                        stand.x,
                        stand.z,
                        door.x,
                        door.z,
                        width,
                        height,
                        i32::from(angle),
                        i32::from(shape),
                        0,
                    ),
                    "route {name}: door {} stand ({},{}) fails the reach rule (cross=({},{})",
                    door.loc,
                    stand.x,
                    stand.z,
                    cross_to.x,
                    cross_to.z,
                );
            }
        }
    }
}

#[test]
fn golden_routes() {
    let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/routes.txt"))
        .expect("routes.txt");
    // One shared world: squares stay painted, every plan timed warm.
    let (world, tables) = setup();
    // Untimed warmup so the first timed plan does not pay one-time IO.
    let _ = plan::plan(
        &world,
        &tables,
        Tile { x: 3222, z: 3222, level: 0 },
        Tile { x: 3223, z: 3222, level: 0 },
        &NavOptions::default(),
    );
    let mut seen: HashSet<String> = HashSet::new();
    let mut count = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split('|').next().map(str::trim).unwrap_or("");
        assert!(seen.insert(name.to_string()), "duplicate route {name}");
        check_route(&world, &tables, line);
        count += 1;
    }
    assert!(count >= 15, "need at least 15 golden routes, got {count}");
    println!("{count} golden routes green");
}
