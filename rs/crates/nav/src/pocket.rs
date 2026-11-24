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
//! The check floods backwards from the destination over the most
//! permissive view the planner could ever use (every door with an open
//! op swung, tolls and requirements ignored, no F2P bound), keeping the
//! planner's own step rule (a door-dependent step needs the door on one
//! of its tiles; one-way doors refuse their shut side) and following
//! ride and custom edges back to their stands, capped at `POCKET_CAP`
//! tiles. A flood that closes under the cap without touching the start
//! proves the planner can never enter: every planner move is one of
//! these. A flood that hits the cap proves nothing and the normal search
//! runs.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;
use std::sync::{Arc, Mutex};

use crate::bfs::can_step;
use crate::collision::tile_walkable;
use crate::plan::{IdHash, SharedEdges, Tile, U64Map};
use crate::search::Search;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Tile-key set with the planner's identity hash (no SipHash per probe).
type U64Set = HashSet<u64, BuildHasherDefault<IdHash>>;

/// Largest pocket the flood proves: a room interior is a few dozen
/// tiles, a fenced yard a few hundred; past this the region is a real
/// area and the planner decides.
pub(crate) const POCKET_CAP: usize = 512;

/// Collision with every openable door swung (requirements, tolls and
/// sides ignored): the widest walk the planner could take. Doors the
/// overrides block tile by tile stay shut, as in the planner.
pub(crate) fn permissive_flags(world: &StaticWorld, tables: &Tables, tile: Tile) -> u32 {
    let options = crate::plan::NavOptions::default();
    Search::permissive(world, tables, &options).open(tile)
}

/// Start-independent flood verdict of one destination tile. `Closed`
/// keeps every tile a start could stand on and still enter the pocket:
/// the walkable members (any level, through ride and custom stands) plus
/// the unwalkable tiles that step in (the original check stopped on
/// `next == from` before the walkable test).
pub(crate) enum PocketVerdict {
    /// Unwalkable destination, or a flood past `POCKET_CAP`: never sealed.
    Open,
    Closed { members: U64Set },
}

/// Flood reads shared by one verdict: the permissive view of the one
/// search reader (`search::Search::permissive`), cached per tile (a
/// tile's door is read by its 9 neighbours').
struct FloodView<'a> {
    world: &'a StaticWorld,
    tables: &'a Tables,
    search: Search<'a>,
}

impl FloodView<'_> {
    /// Every door with an open op swung, except tile-blocked ones.
    fn permissive(&mut self, tile: Tile) -> u32 {
        self.search.open(tile)
    }

    /// A door on this tile the planner could ever swing (open op, not
    /// blocked on this tile).
    fn openable(&mut self, tile: Tile) -> Option<u16> {
        self.search.usable_door(tile).map(|(id, _, _)| id)
    }

    /// The planner's own move rule (`plan::dijkstra`), widened only where
    /// it depends on the search options: `from` steps onto `to` when the
    /// shut view allows it and `to` holds no door, or through an
    /// openable door on one of the two tiles. A one-way door refuses a
    /// stand off its passable side (`Tables::side_ok`), for the click
    /// edge and for walking onto its tile alike; stepping off the door
    /// tile itself continues an opening and is never refused. A one-way
    /// door the planner may read as shut (toll, requirement) keeps the
    /// plain walk onto its tile.
    fn planner_may_step(&mut self, from: Tile, to: Tile) -> bool {
        let search = &mut self.search;
        let mut shut = |x: i32, z: i32| search.base(Tile { x, z, level: to.level });
        let shut_step = can_step(&mut shut, from.x, from.z, to.x, to.z);
        if let Some(id) = self.openable(to) {
            if self.tables.side_ok(id, to, from) {
                return true;
            }
            let conditional = self.world.is_toll(to.x, to.z, to.level) || self.tables.requirements.iter().any(|row| row.at == to);
            return conditional && shut_step;
        }
        shut_step || self.openable(from).is_some()
    }
}

/// Flood from `dest` with no start: the same walk as the per-start check,
/// minus its early `next == from` exit, so one verdict answers every start.
/// Rides and custom edges also count backwards: a ride landing in the
/// pocket floods its stand (any level), so a fortress whose upper floor
/// is entered only from inside stays sealed (Black Knights' Fortress
/// 2026-11-17: ladders (3022,3517) / (3011,3514) land inside, their
/// stands are upstairs, reached only by the ladders from inside).
pub(crate) fn verdict(world: &StaticWorld, tables: &Tables, shared: &SharedEdges, dest: Tile) -> PocketVerdict {
    let options = crate::plan::NavOptions::default();
    let mut view = FloodView { world, tables, search: Search::permissive(world, tables, &options) };
    if !tile_walkable(view.permissive(dest)) {
        return PocketVerdict::Open;
    }
    let (rides, customs) = shared;
    let mut stands_into: U64Map<Vec<u64>> = U64Map::default();
    for (stand, landing) in rides
        .iter()
        .flat_map(|(stand, edges)| edges.iter().map(move |edge| (*stand, edge.dest.key())))
        .chain(customs.iter().flat_map(|(stand, edges)| edges.iter().map(move |edge| (*stand, edge.dest.key()))))
    {
        stands_into.entry(landing).or_default().push(stand);
    }
    let mut seen = U64Set::default();
    let mut entries = U64Set::default();
    seen.insert(dest.key());
    let mut queue = vec![dest];
    while let Some(here) = queue.pop() {
        for stand in stands_into.get(&here.key()).into_iter().flatten() {
            if seen.insert(*stand) {
                if seen.len() > POCKET_CAP {
                    return PocketVerdict::Open;
                }
                queue.push(crate::plan::unkey(*stand));
            }
        }
        let level = here.level;
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level };
            if seen.contains(&next.key()) {
                continue;
            }
            // Backward over the planner's own move: `next` steps onto
            // `here` (the server rule reads the entered tile only). A
            // tile no step can enter (loc, blocked floor) is never on a
            // path, except as the start itself.
            let mut step = |x: i32, z: i32| view.permissive(Tile { x, z, level });
            if !can_step(&mut step, next.x, next.z, here.x, here.z) {
                continue;
            }
            if !view.planner_may_step(next, here) {
                continue;
            }
            if !tile_walkable(view.permissive(next)) {
                entries.insert(next.key());
                continue;
            }
            seen.insert(next.key());
            if seen.len() > POCKET_CAP {
                return PocketVerdict::Open;
            }
            queue.push(next);
        }
    }
    seen.extend(entries);
    PocketVerdict::Closed { members: seen }
}

/// The per-start answer read off a start-independent verdict.
pub(crate) fn sealed_by(verdict: &PocketVerdict, from: Tile, dest: Tile) -> bool {
    if from == dest {
        return false;
    }
    match verdict {
        PocketVerdict::Open => false,
        PocketVerdict::Closed { members } => !members.contains(&from.key()),
    }
}

/// True when `dest` is walkable and sealed: no walk (doors swung), ride
/// or custom edge can reach it from outside, and `from` is not inside.
/// The planner's verdict on such a destination is always `Disconnected`.
pub(crate) fn sealed(world: &StaticWorld, tables: &Tables, shared: &SharedEdges, from: Tile, dest: Tile) -> bool {
    if from == dest {
        return false;
    }
    sealed_by(&verdict(world, tables, shared, dest), from, dest)
}

/// Verdicts kept past one search (CPU 2026-11-14): the flood is static
/// per snapshot version, and every plan miss and every `walk_costs`
/// target paid it again (8 floods of up to 512 tiles per NPC ring). The
/// service owns one memo per L2 version; bounded (cleared when full).
#[derive(Default)]
pub struct PocketMemo {
    verdicts: Mutex<HashMap<u64, Arc<PocketVerdict>>>,
}

/// Memo bound: a verdict is a few hundred tile keys at most.
const MEMO_CAP: usize = 65_536;

impl PocketMemo {
    pub(crate) fn sealed(&self, world: &StaticWorld, tables: &Tables, shared: &SharedEdges, from: Tile, dest: Tile) -> bool {
        if from == dest {
            return false;
        }
        let known = self.verdicts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&dest.key()).cloned();
        let found = match known {
            Some(found) => found,
            None => {
                // Computed outside the lock: two threads may flood the
                // same tile once each, never block each other.
                let fresh = Arc::new(verdict(world, tables, shared, dest));
                let mut verdicts = self.verdicts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if verdicts.len() >= MEMO_CAP {
                    verdicts.clear();
                }
                verdicts.insert(dest.key(), Arc::clone(&fresh));
                fresh
            }
        };
        sealed_by(&found, from, dest)
    }

    pub fn len(&self) -> usize {
        self.verdicts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Sealed check through the memo when the caller has one.
pub(crate) fn sealed_with(memo: Option<&PocketMemo>, world: &StaticWorld, tables: &Tables, shared: &SharedEdges, from: Tile, dest: Tile) -> bool {
    match memo {
        Some(memo) => memo.sealed(world, tables, shared, from, dest),
        None => sealed(world, tables, shared, from, dest),
    }
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
        world.set_npc_blockers(tables.npc_blockers.clone());
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
        // The memo answers exactly like the per-start flood, for starts
        // outside, inside and on the pocket's unwalkable rim.
        let memo = super::PocketMemo::default();
        for (from, dest) in [
            (tile(3245, 3395, 0), tile(3247, 3398, 0)),
            (tile(3247, 3398, 0), tile(3247, 3398, 0)),
            (tile(3246, 3396, 0), tile(3246, 3396, 0)),
            (tile(3222, 3222, 0), tile(3246, 3396, 0)),
            (tile(3257, 3244, 0), tile(3257, 3242, 0)),
            (tile(3257, 3242, 0), tile(3257, 3243, 0)),
        ] {
            for start in [from, tile(from.x + 1, from.z, 0), tile(dest.x, dest.z + 1, 0), tile(dest.x - 1, dest.z, 0)] {
                assert_eq!(memo.sealed(&world, &tables, &shared, start, dest), sealed(&world, &tables, &shared, start, dest), "{start:?} -> {dest:?}");
            }
        }
        assert!(!memo.is_empty(), "verdicts kept");
    }

    /// Live tim_thorn-105223: Varrock west bank to the air temple floor
    /// (entered only by a talisman on the ruins) is a sealed destination.
    /// Its NoRoute must not name the start as the closest tile: the brain
    /// reads a closest beside the start as a walled-in origin and held a
    /// bot standing at a bank (`brain_stranded`).
    #[test]
    fn a_sealed_destination_never_names_the_start_closest() {
        let (world, tables) = setup();
        let options = NavOptions { f2p_only: true, ..NavOptions::default() };
        let (bank, temple_floor) = (tile(3185, 3436, 0), tile(2841, 4829, 0));
        let refused = crate::plan::plan(&world, &tables, bank, temple_floor, &options).expect_err("the temple floor is sealed");
        assert_eq!(refused.reason, crate::plan::NoRouteReason::Disconnected);
        assert_eq!(refused.closest_reachable, temple_floor, "the verdict is about the destination");
    }
}
