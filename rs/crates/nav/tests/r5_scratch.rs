//! Scratch R5 pocket diagnostics (offline). Prints plan outcomes +
//! nearby doors/flags. Deleted before the final diff.

use std::sync::Arc;

use lifescape_nav::plan::{self, NavOptions, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

fn workspace_root() -> std::path::PathBuf {
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

fn probe(world: &Arc<StaticWorld>, tables: &Tables, name: &str, from: Tile, dest: Tile) {
    for f2p in [true, false] {
        let mut options = NavOptions::default();
        options.f2p_only = f2p;
        match plan::plan(world, tables, from, dest, &options) {
            Ok(plan) => println!(
                "{name} f2p={f2p}: OK segs={} cost={} adjusted={:?}",
                plan.segments.len(),
                plan.cost,
                plan.adjusted_dest
            ),
            Err(no) => println!(
                "{name} f2p={f2p}: NoRoute {} closest=({},{},{})",
                no.reason.as_str(),
                no.closest_reachable.x,
                no.closest_reachable.z,
                no.closest_reachable.level
            ),
        }
    }
}

#[test]
fn r5_probe() {
    let (world, tables) = setup();
    // proof-a exact + duke ground + juliet exact + alkharid f2p splits.
    probe(&world, &tables, "proofa-down", Tile { x: 2970, z: 3376, level: 1 }, Tile { x: 2971, z: 3369, level: 0 });
    probe(&world, &tables, "duke-ground", Tile { x: 3222, z: 3218, level: 0 }, Tile { x: 3212, z: 3220, level: 1 });
    probe(&world, &tables, "duke", Tile { x: 3227, z: 3213, level: 1 }, Tile { x: 3212, z: 3220, level: 1 });
    probe(&world, &tables, "juliet-admin272", Tile { x: 3210, z: 3424, level: 0 }, Tile { x: 3158, z: 3425, level: 1 });
    probe(&world, &tables, "juliet-land", Tile { x: 3175, z: 3419, level: 1 }, Tile { x: 3162, z: 3425, level: 1 });
    probe(&world, &tables, "juliet-L0stairs", Tile { x: 3210, z: 3424, level: 0 }, Tile { x: 3175, z: 3420, level: 0 });
    probe(&world, &tables, "juliet-L0short", Tile { x: 3189, z: 3428, level: 0 }, Tile { x: 3175, z: 3420, level: 0 });
    probe(&world, &tables, "alkharid-t02-705", Tile { x: 3257, z: 3173, level: 0 }, Tile { x: 3210, z: 3213, level: 0 });
    probe(&world, &tables, "alkharid-east", Tile { x: 3273, z: 3176, level: 0 }, Tile { x: 3210, z: 3213, level: 0 });
    probe(&world, &tables, "alkharid-pocket", Tile { x: 3273, z: 3176, level: 0 }, Tile { x: 3257, z: 3173, level: 0 });
    probe(&world, &tables, "cook-t02-1916", Tile { x: 3204, z: 3211, level: 0 }, Tile { x: 3209, z: 3213, level: 0 });
    for (x, z) in [(3257, 3173), (3273, 3176), (3210, 3213), (3227, 3213), (3212, 3220), (3175, 3419)] {
        println!("in_f2p ({x},{z}) = {}", world.in_f2p(x, z));
    }
    // Cook pocket: which neighbours of from/dest plan OK?
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (2, 0), (0, 2), (5, 2), (6, 2)] {
        probe(
            &world,
            &tables,
            &format!("cook-step-{dx},{dz}"),
            Tile { x: 3204, z: 3211, level: 0 },
            Tile { x: 3204 + dx, z: 3211 + dz, level: 0 },
        );
    }
    // Doors in the suspect rectangles: loc + shape/angle + open_op + side rule.
    for (name, x0, x1, z0, z1, l) in [
        ("kitchen", 3200, 3220, 3208, 3220, 0),
        ("julietL0", 3155, 3200, 3420, 3432, 0),
        ("alkharid-north", 3250, 3280, 3180, 3235, 0),
        ("dukeL1", 3210, 3232, 3208, 3226, 1),
        ("julietL1", 3155, 3180, 3414, 3428, 1),
    ] {
        println!("--- doors {name}");
        for x in x0..=x1 {
            for z in z0..=z1 {
                if let Some((id, shape, angle)) = world.door_at(x, z, l) {
                    let open_op = world.loc_def(id as usize).and_then(|def| def.open_op);
                    let toll = world.is_toll(x, z, l);
                    println!("door {id} at ({x},{z},{l}) shape={shape} angle={angle} open_op={open_op:?} toll={toll}");
                }
            }
        }
    }
    for (name, sx, sz, sl) in [
        ("comp-duke", 3227, 3213, 1),
        ("comp-julietL1", 3174, 3419, 1),
        ("comp-julietL0", 3189, 3428, 0),
        ("comp-cook", 3204, 3211, 0),
        ("comp-alkharid", 3257, 3173, 0),
        ("comp-fallyL1", 2970, 3376, 1),
    ] {
        let mut seen: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();
        let mut stack: Vec<(i32, i32)> = vec![(sx, sz)];
        seen.insert((sx, sz));
        while let Some((x, z)) = stack.pop() {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, nz) = (x + dx, z + dz);
                if seen.contains(&(nx, nz)) {
                    continue;
                }
                if (nx - sx).abs() + (nz - sz).abs() > 60 {
                    continue;
                }
                // Walkable in the planning view (doors open).
                if world.door_at(nx, nz, sl).is_some()
                    || lifescape_nav::collision::tile_walkable(world.get(nx, nz, sl))
                {
                    seen.insert((nx, nz));
                    stack.push((nx, nz));
                }
            }
        }
        let (mut x0, mut x1, mut z0, mut z1) = (sx, sx, sz, sz);
        for (x, z) in &seen {
            x0 = x0.min(*x);
            x1 = x1.max(*x);
            z0 = z0.min(*z);
            z1 = z1.max(*z);
        }
        println!("{name}: size={} x {x0}..={x1} z {z0}..={z1}", seen.len());
    }
    // Juliet seal binary search: where westward dies.
    for (x, z) in [(3200, 3428), (3195, 3428), (3190, 3428), (3185, 3428), (3180, 3428), (3175, 3428), (3170, 3428), (3165, 3428), (3160, 3428)] {
        probe(&world, &tables, &format!("juliet-seal-{x},{z}"), Tile { x: 3210, z: 3424, level: 0 }, Tile { x, z, level: 0 });
    }
    // Ride stands for the down stair + its verified arrival.
    let options = NavOptions::default();
    for name in ["stairstop_336", "stairs_316", "stairs_327"] {
        let Some(row) = tables.transitions.iter().find(|row| row.name == name) else {
            println!("{name}: no row");
            continue;
        };
        let stands = plan::ride_stands(&world, &tables, &options, row);
        println!("{name}: stands={stands:?} proven={:?}", tables.proven_arrival(row.loc, row.from));
    }
    // Doors near castle L1 duke room + juliet + cook + door 1536 tiles.
    for (x, z, l) in [(3227, 3213, 1), (3212, 3220, 1), (3215, 3218, 1), (3162, 3425, 1), (3175, 3419, 1), (3209, 3213, 0), (3204, 3211, 0), (3109, 3159, 1)] {
        println!("door_at ({x},{z},{l}) = {:?}", world.door_at(x, z, l));
    }
    // Walkability grids: # = blocked, . = open, D = door.
    for (name, x0, x1, z0, z1, l) in [
        ("castle-L1-east", 3220, 3232, 3208, 3225, 1),
        ("juliet-L1", 3168, 3180, 3414, 3428, 1),
        ("juliet-L0", 3185, 3195, 3424, 3432, 0),
        ("alkharid-south", 3250, 3262, 3166, 3180, 0),
        ("cook-L0", 3202, 3212, 3209, 3216, 0),
        ("fally-L1-stairs", 2968, 2976, 3368, 3376, 1),
        ("fally-L0-landing", 2966, 2976, 3362, 3372, 0),
        ("juliet-L0-wide", 3155, 3215, 3420, 3432, 0),
        ("alkharid-wide", 3250, 3280, 3166, 3180, 0),
        ("castle-L0-duke", 3205, 3230, 3210, 3228, 0),
    ] {
        println!("--- {name} (x {x0}..={x1}, z {z0}..={z1}, L{l})");
        for z in (z0..=z1).rev() {
            let mut row = String::new();
            for x in x0..=x1 {
                let flags = world.get(x, z, l);
                let ch = if world.door_at(x, z, l).is_some() {
                    'D'
                } else if lifescape_nav::collision::tile_walkable(flags) {
                    '.'
                } else {
                    '#'
                };
                row.push(ch);
            }
            println!("z={z:>4} {row}");
        }
    }
    // Where is loc 1536 in the static world?
    let mut found = 0;
    for x in 3080..3260 {
        for z in 3140..3260 {
            for l in 0..2 {
                if let Some((id, _, _)) = world.door_at(x, z, l) {
                    if id == 1536 {
                        println!("door 1536 at ({x},{z},{l})");
                        found += 1;
                        if found > 12 {
                            break;
                        }
                    }
                }
            }
        }
    }
    println!("done");
}
