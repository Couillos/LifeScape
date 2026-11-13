//! Sealed pockets: a walkable destination nothing can enter.
//!
//! A tile can be walkable (no loc on it) yet walled in on every side:
//! a corner between a table, a stool and two house walls, the strip
//! behind a garden fence, a window bay between shelves. Callers aim at
//! such tiles when they ring an NPC (`approach_npc`) or pick a click tile
//! without a reachability check. The planner then has no early exit: the
//! verdict search explores the whole reachable F2P world (~239k visits,
//! 3-9 s on the daemon debug build), the diagnosis searches run again,
//! and the answer is `Disconnected` anyway. Live 2026-11-13: 21 melee
//! NoRoute dossiers in 40 min, each 4-6 s of a worker blocked on the
//! plan (Varrock houses (3247,3398), (3236,3409), Falador (2970,3372),
//! (3004,3323), Lumbridge fence (3257,3242)).
//!
//! The check floods from the destination over the most permissive view
//! the planner could ever use (every door with an open op swung, tolls
//! and requirements ignored, no F2P bound, both step directions), capped
//! at `POCKET_CAP` tiles. A flood that closes under the cap without
//! touching the start, and that no ride or custom edge lands in, proves
//! the planner can never enter: every planner move is one of these. A
//! flood that hits the cap proves nothing and the normal search runs.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use crate::bfs::can_step;
use crate::collision::{tile_walkable, CollisionMap};
use crate::plan::{SharedEdges, Tile};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Largest pocket the flood proves: a room interior is a few dozen
/// tiles, a fenced yard a few hundred; past this the region is a real
/// area and the planner decides.
pub(crate) const POCKET_CAP: usize = 512;

/// Collision with every openable door swung (requirements, tolls and
/// sides ignored): the widest walk the planner could take. Doors the
/// overrides block tile by tile stay shut, as in the planner.
fn permissive_flags(world: &StaticWorld, tables: &Tables, tile: Tile) -> u32 {
    let mut doors = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
            let Some((id, shape, angle)) = world.door_at(near.x, near.z, near.level) else {
                continue;
            };
            if tables.is_blocked(id, near) {
                continue;
            }
            if world.loc_def(id as usize).and_then(|def| def.open_op).is_none() {
                continue;
            }
            doors.push((near, id, shape, angle));
        }
    }
    if doors.is_empty() {
        return world.get(tile.x, tile.z, tile.level);
    }
    let mut scratch = CollisionMap::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
            scratch.set(near.x, near.z, near.level, world.get(near.x, near.z, near.level));
        }
    }
    for (near, id, shape, angle) in doors {
        world.unpaint_into(&mut scratch, id, shape, angle, near.x, near.z, near.level);
    }
    scratch.get(tile.x, tile.z, tile.level)
}

/// True when `dest` is walkable and sealed: no walk (doors swung), ride
/// or custom edge can reach it from outside, and `from` is not inside.
/// The planner's verdict on such a destination is always `Disconnected`.
pub(crate) fn sealed(world: &StaticWorld, tables: &Tables, shared: &SharedEdges, from: Tile, dest: Tile) -> bool {
    if from == dest {
        return false;
    }
    let mut flags = std::collections::HashMap::new();
    let mut view = |tile: Tile| -> u32 {
        *flags.entry(tile.key()).or_insert_with(|| permissive_flags(world, tables, tile))
    };
    if !tile_walkable(view(dest)) {
        return false;
    }
    let level = dest.level;
    let mut seen: HashSet<u64> = HashSet::new();
    seen.insert(dest.key());
    let mut queue = vec![dest];
    while let Some(here) = queue.pop() {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level };
            if seen.contains(&next.key()) {
                continue;
            }
            // Backward over the planner's own move: `next` steps onto
            // `here` (the server rule reads the entered tile only). A
            // tile no step can enter (loc, blocked floor) is never on a
            // path, except as the start itself.
            let mut step = |x: i32, z: i32| view(Tile { x, z, level });
            if !can_step(&mut step, next.x, next.z, here.x, here.z) {
                continue;
            }
            if next == from {
                return false;
            }
            if !tile_walkable(view(next)) {
                continue;
            }
            seen.insert(next.key());
            if seen.len() > POCKET_CAP {
                return false;
            }
            queue.push(next);
        }
    }
    let (rides, customs) = shared;
    let ride_in = rides.values().flatten().any(|edge| seen.contains(&edge.dest.key()));
    let custom_in = customs.values().flatten().any(|edge| seen.contains(&edge.dest.key()));
    !ride_in && !custom_in
}

#[cfg(test)]
mod tests {
    use super::sealed;
    use crate::plan::{prepare_shared, NavOptions, Tile};

    fn setup() -> (std::sync::Arc<crate::static_world::StaticWorld>, crate::tables::Tables) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = crate::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        (world, tables)
    }

    fn tile(x: i32, z: i32, level: i32) -> Tile {
        Tile { x, z, level }
    }

    /// The live NoRoute melee destinations (2026-11-13 dossiers) are
    /// sealed; their neighbours across the same rooms, a door-only room,
    /// a ladder landing and a start inside the pocket are not.
    #[test]
    fn live_pockets_are_sealed_and_rooms_are_not() {
        let (world, tables) = setup();
        let options = NavOptions { f2p_only: true, ..NavOptions::default() };
        let shared = prepare_shared(&world, &tables, &options, &mut || true).expect("edges");
        for (from, dest) in [
            (tile(3245, 3395, 0), tile(3247, 3398, 0)),
            (tile(3241, 3408, 0), tile(3236, 3409, 0)),
            (tile(3239, 3406, 0), tile(3240, 3403, 0)),
            (tile(2979, 3381, 0), tile(2970, 3372, 0)),
            (tile(3005, 3323, 0), tile(3004, 3323, 0)),
            (tile(3257, 3244, 0), tile(3257, 3242, 0)),
        ] {
            assert!(sealed(&world, &tables, &shared, from, dest), "{dest:?} is a sealed pocket");
        }
        for (from, dest, why) in [
            (tile(3245, 3395, 0), tile(3246, 3396, 0), "same house, open floor"),
            (tile(3222, 3222, 0), tile(3246, 3396, 0), "Varrock house behind its door 1530"),
            (tile(3074, 3424, 0), tile(3075, 3429, 0), "Peksa's shop, door-only room"),
            (tile(3222, 3222, 0), tile(3229, 3212, 2), "castle tower top, ladder landing"),
            (tile(3247, 3398, 0), tile(3247, 3398, 0), "start is the destination"),
        ] {
            assert!(!sealed(&world, &tables, &shared, from, dest), "{why}: {dest:?} is enterable");
        }
    }
}
