//! The dense grid against the paged reader it caches (lot N2): every
//! tile of every painted square, every level, plus the squares the pack
//! does not ship. Needs the engine pack like the other world tests.

use std::sync::Arc;

use crate::collision::{NULL, OPEN};
use crate::plan::Tile;
use crate::reader::Reader;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

fn world_and_tables() -> (Arc<StaticWorld>, Tables) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
    let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    world.set_f2p(crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p"));
    let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    world.set_npc_blockers(tables.npc_blockers.clone());
    (world, tables)
}

/// Compare one built square with the paged reader, tile by tile.
fn compare_square(world: &StaticWorld, square_x: i32, square_z: i32, level: i32, mismatches: &mut Vec<String>) -> usize {
    let grid = world.grid();
    let reader = Reader::new(world);
    let mut checked = 0;
    let mut door_tiles = 0;
    for z in 0..64 {
        for x in 0..64 {
            let (tile_x, tile_z) = ((square_x << 6) + x, (square_z << 6) + z);
            let tile = Tile { x: tile_x, z: tile_z, level };
            let square = grid.square(world, tile_x, tile_z, level).expect("square builds");
            let paged = world.get_paged(tile_x, tile_z, level);
            if square.flags(tile_x, tile_z) != paged || reader.flags(tile) != paged || world.get(tile_x, tile_z, level) != paged {
                mismatches.push(format!("flags {tile:?}: grid {:#x} paged {paged:#x}", square.flags(tile_x, tile_z)));
            }
            let door = world.door_at(tile_x, tile_z, level);
            if square.has_door(tile_x, tile_z) != door.is_some() || reader.door_at(tile) != door {
                mismatches.push(format!("door {tile:?}: grid {} paged {door:?}", square.has_door(tile_x, tile_z)));
            }
            if door.is_some() {
                door_tiles += 1;
            }
            let inside = world.in_f2p(tile_x, tile_z);
            if square.in_f2p(tile_x, tile_z) != inside || reader.in_f2p(tile) != inside {
                mismatches.push(format!("f2p {tile:?}: grid {} paged {inside}", square.in_f2p(tile_x, tile_z)));
            }
            // Door on the 3x3: exact off the border, conservative on it.
            let border = x == 0 || z == 0 || x == 63 || z == 63;
            let any_door = (-1..=1).any(|dz| (-1..=1).any(|dx| world.door_at(tile_x + dx, tile_z + dz, level).is_some()));
            let near = square.near_door(tile_x, tile_z);
            if (border && !near) || (!border && near != any_door) || (any_door && !near) {
                mismatches.push(format!("near_door {tile:?}: grid {near} paged {any_door}"));
            }
            checked += 1;
        }
    }
    let _ = door_tiles;
    checked
}

#[test]
fn grid_equals_the_paged_reader_on_every_tile_of_every_pack_square() {
    let (world, _tables) = world_and_tables();
    let mut squares = world_squares(&world);
    squares.sort_unstable();
    assert!(squares.len() > 400, "the pack ships {} squares", squares.len());
    let (mut checked, mut mismatches) = (0usize, Vec::new());
    for (square_x, square_z) in &squares {
        for level in 0..4 {
            checked += compare_square(&world, *square_x, *square_z, level, &mut mismatches);
        }
    }
    println!("grid vs paged: {} squares x 4 levels, {checked} tiles, {} mismatches", squares.len(), mismatches.len());
    assert!(mismatches.is_empty(), "first mismatches: {:?}", &mismatches[..mismatches.len().min(10)]);
}

fn world_squares(world: &StaticWorld) -> Vec<(i32, i32)> {
    (0..256).flat_map(|square_x| (0..256).map(move |square_z| (square_x, square_z))).filter(|(sx, sz)| world.has_square(*sx, *sz)).collect()
}

#[test]
fn grid_equals_the_paged_reader_on_squares_the_pack_lacks() {
    let (world, tables) = world_and_tables();
    let missing: Vec<(i32, i32)> = (0..40)
        .flat_map(|square_x| (0..8).map(move |square_z| (square_x, square_z)))
        .filter(|(sx, sz)| !world.has_square(*sx, *sz))
        .take(6)
        .collect();
    assert!(!missing.is_empty());
    let mut mismatches = Vec::new();
    for (square_x, square_z) in &missing {
        for level in 0..4 {
            compare_square(&world, *square_x, *square_z, level, &mut mismatches);
        }
    }
    // Ground below, void above, except the allowlisted real floors.
    let grid = world.grid();
    let (square_x, square_z) = missing[0];
    let sky = tables.sky_floors.iter().find(|(x, z, level)| *level > 0 && !world.has_square(x >> 6, z >> 6));
    if let Some((x, z, level)) = sky {
        assert_eq!(grid.square(&world, *x, *z, *level).unwrap().flags(*x, *z), OPEN);
    }
    assert_eq!(grid.square(&world, square_x << 6, square_z << 6, 1).unwrap().flags(square_x << 6, square_z << 6), NULL);
    assert!(mismatches.is_empty(), "{:?}", &mismatches[..mismatches.len().min(10)]);
}

#[test]
fn tiles_outside_the_grid_fall_back_to_the_paged_reader() {
    let (world, _tables) = world_and_tables();
    let reader = Reader::new(&world);
    for tile in [Tile { x: -5, z: 3200, level: 0 }, Tile { x: 3200, z: -1, level: 0 }, Tile { x: 20_000, z: 100, level: 0 }, Tile { x: 3200, z: 3200, level: 4 }, Tile { x: 3200, z: 3200, level: -1 }] {
        assert_eq!(reader.flags(tile), world.get_paged(tile.x, tile.z, tile.level), "{tile:?}");
        assert_eq!(world.get(tile.x, tile.z, tile.level), world.get_paged(tile.x, tile.z, tile.level), "{tile:?}");
        assert_eq!(reader.door_at(tile), world.door_at(tile.x, tile.z, tile.level), "{tile:?}");
    }
}

#[test]
fn a_setter_drops_the_built_squares() {
    let (world, tables) = world_and_tables();
    let before = world.get(3222, 3218, 0);
    assert_eq!(world.grid().built(), 1);
    world.set_sky_floors(tables.sky_floors.clone());
    assert_eq!(world.grid().built(), 0);
    assert_eq!(world.get(3222, 3218, 0), before);
}

/// Painting order and a square's edge: a loc anchored in a neighbour
/// square (a wall, a multi-tile object) paints tiles that lie in this
/// one, so a tile near the edge is only final once the neighbour is
/// painted. The paged reader answers the alone-painted value until the
/// neighbour happens to be touched (order dependent, a latent flaw: 386
/// tiles on 33 sampled squares). The dense square is built after its 8
/// neighbours are painted, so it always serves the final value. This
/// test counts the tiles where the two differ and pins how deep from the
/// edge they reach (a loc is at most a few tiles wide).
#[test]
fn painting_order_only_touches_the_edge() {
    let (final_world, _tables) = world_and_tables();
    let mut squares = world_squares(&final_world);
    squares.sort_unstable();
    let (mut differing, mut sampled, mut deepest) = (0usize, 0usize, 0i32);
    for (square_x, square_z) in squares.iter().step_by(15) {
        let (alone, _) = world_and_tables();
        sampled += 1;
        for level in 0..4 {
            for z in 0..64 {
                for x in 0..64 {
                    let (tile_x, tile_z) = ((square_x << 6) + x, (square_z << 6) + z);
                    if alone.get_paged(tile_x, tile_z, level) != final_world.get(tile_x, tile_z, level) {
                        differing += 1;
                        deepest = deepest.max(x.min(63 - x).min(z).min(63 - z));
                    }
                }
            }
        }
    }
    println!("paint order: {sampled} squares sampled, {differing} tiles differ, deepest {deepest} tiles from the edge");
    assert!(deepest <= 8, "painting order reaches {deepest} tiles inside a square");
}

#[test]
#[ignore]
fn cold_cost_probe() {
    let (world, _tables) = world_and_tables();
    let start = std::time::Instant::now();
    let _ = world.get_paged(3031, 3516, 0);
    println!("paint one square (paged): {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let _ = world.get(3031, 3516, 0);
    println!("build level 0 (neighbours painted): {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let _ = world.get(3031, 3516, 1);
    println!("build level 1: {:?}", start.elapsed());
    let start = std::time::Instant::now();
    let _ = world.get(3100, 3516, 0);
    println!("build next square level 0: {:?}", start.elapsed());
}
