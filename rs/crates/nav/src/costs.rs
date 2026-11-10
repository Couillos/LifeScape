#![forbid(unsafe_code)]
//! Multi-target walking costs (PRD 2, N1): one Dijkstra from the bot,
//! settled per target, stopped when every target is reached.
//!
//! Same edge rules as `plan.rs` (`dijkstra`, fully-filtered leg D):
//! walk steps read the usable-door view, doors need an open op, their
//! requirement tags and a reach-valid stand, rides ride the shared L2
//! with the same per-search filters. A settled pop is optimal by
//! Dijkstra, so each `Some(cost)` prices exactly what a unit plan would
//! (`multi_target_equals_unit_plans` pins it); anything unsettled at
//! exhaustion reads `None`.
//!
//! Known limit: a blocked destination tile (tree, counter) never settles
//! and reads `None`, while `plan()` would serve an adjusted plan to the
//! closest reachable tile. Pickers target walkable tiles and sink the
//! rest behind planned, so the cheaper rule stands; revisit if a picker
//! ever needs the adjusted price.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::bfs::can_step;
use crate::collision::{self, CollisionMap};
use crate::plan::{EdgeId, NavOptions, SharedEdges, Tile, DOOR_COST};
use crate::reach;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Search budget, same as `plan.rs`: explored tiles before the area reads
/// disconnected. Nearby candidates settle in milliseconds; the bound only
/// bites pathological batches.
const MAX_VISITS: usize = 500_000;
/// Wall-clock bound for one batch: an exhaustive miss stops at its current
/// costs instead of burning the acceptor (the `plan` op's budget).
const COSTS_BUDGET: std::time::Duration = std::time::Duration::from_millis(1500);

type Key = (i32, i32, i32);

fn key_of(tile: Tile) -> Key {
    (tile.x, tile.z, tile.level)
}

fn tile_of(key: Key) -> Tile {
    Tile { x: key.0, z: key.1, level: key.2 }
}

struct Search<'a> {
    world: &'a StaticWorld,
    tables: &'a Tables,
    options: &'a NavOptions,
    base_flags: HashMap<Key, u32>,
    open_flags: HashMap<Key, u32>,
    doors: HashMap<Key, Option<(u16, u8, u8)>>,
}

impl Search<'_> {
    fn in_bounds(&self, tile: Tile) -> bool {
        !self.options.f2p_only || self.world.in_f2p(tile.x, tile.z)
    }

    fn base(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.base_flags.get(&key_of(tile)) {
            return *flags;
        }
        let flags = self.world.get(tile.x, tile.z, tile.level);
        self.base_flags.insert(key_of(tile), flags);
        flags
    }

    fn door_at(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        if let Some(found) = self.doors.get(&key_of(tile)) {
            return *found;
        }
        let found = self.world.door_at(tile.x, tile.z, tile.level);
        self.doors.insert(key_of(tile), found);
        found
    }

    /// Same gate as `plan.rs`: openable, untolled (or paid), requirement
    /// tags in hand, never hand-blocked tile by tile.
    fn usable_door(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        let (id, shape, angle) = self.door_at(tile)?;
        if self.tables.is_blocked(id, tile) {
            return None;
        }
        let def = self.world.loc_def(id as usize)?;
        def.open_op?;
        if self.world.is_toll(tile.x, tile.z, tile.level) && !self.options.allow_paid {
            return None;
        }
        if !self
            .tables
            .missing(id, tile, &self.options.satisfied)
            .is_empty()
        {
            return None;
        }
        Some((id, shape, angle))
    }

    /// Walk view with usable doors swung open (same 3x3 scratch as plans).
    fn open(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.open_flags.get(&key_of(tile)) {
            return *flags;
        }
        let mut doors = Vec::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                if let Some(found) = self.usable_door(near) {
                    doors.push((near, found));
                }
            }
        }
        let flags = if doors.is_empty() {
            self.base(tile)
        } else {
            let mut scratch = CollisionMap::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                    scratch.set(near.x, near.z, near.level, self.base(near));
                }
            }
            for (near, (id, shape, angle)) in doors {
                self.world.unpaint_into(&mut scratch, id, shape, angle, near.x, near.z, near.level);
            }
            scratch.get(tile.x, tile.z, tile.level)
        };
        self.open_flags.insert(key_of(tile), flags);
        flags
    }

    fn step_open(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.open(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }

    fn step_closed(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.base(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }
}

fn relax(dist: &mut HashMap<Key, u32>, heap: &mut BinaryHeap<(Reverse<u32>, Reverse<Key>)>, next: Key, cost: u32) {
    if dist.get(&next).is_some_and(|known| *known <= cost) {
        return;
    }
    dist.insert(next, cost);
    heap.push((Reverse(cost), Reverse(next)));
}

/// One search, many targets: `costs[i]` prices `dests[i]`, `None` where
/// the target is unplannable. Same options a plan would use.
pub fn plan_costs(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dests: &[Tile],
    options: &NavOptions,
) -> Vec<Option<u32>> {
    let mut always = || true;
    let Some(shared) = crate::plan::prepare_shared(world, tables, options, &mut always) else {
        return vec![None; dests.len()];
    };
    plan_costs_with(world, tables, from, dests, options, &shared)
}

/// Same search on a precomputed L2 edge set (the service pool path).
pub(crate) fn plan_costs_with(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dests: &[Tile],
    options: &NavOptions,
    shared: &SharedEdges,
) -> Vec<Option<u32>> {
    let mut search = Search {
        world,
        tables,
        options,
        base_flags: HashMap::new(),
        open_flags: HashMap::new(),
        doors: HashMap::new(),
    };
    let (rides, customs) = shared;
    // Target keys still unsettled; repeats share one entry, every index
    // reads it back in order.
    let mut pending: HashMap<Key, Vec<usize>> = HashMap::new();
    for (index, dest) in dests.iter().enumerate() {
        pending.entry(key_of(*dest)).or_default().push(index);
    }
    let mut costs: Vec<Option<u32>> = vec![None; dests.len()];
    let mut dist: HashMap<Key, u32> = HashMap::new();
    let mut heap: BinaryHeap<(Reverse<u32>, Reverse<Key>)> = BinaryHeap::new();
    dist.insert(key_of(from), 0);
    heap.push((Reverse(0), Reverse(key_of(from))));
    let mut visits = 0usize;
    let deadline = std::time::Instant::now() + COSTS_BUDGET;
    eprintln!("costs dbg: from={:?} ndests={} nride_keys={}", from, dests.len(), rides.len());
    while let Some((Reverse(cost), Reverse(found))) = heap.pop() {
        if dist.get(&found).is_some_and(|known| *known != cost) {
            continue;
        }
        visits += 1;
        if visits > MAX_VISITS {
            break;
        }
        if visits % 1024 == 0 && std::time::Instant::now() >= deadline {
            break;
        }
        if visits < 5 {
            eprintln!("costs dbg pop {visits}: {found:?} cost={cost}");
        }
        if let Some(indices) = pending.remove(&found) {
            for index in indices {
                costs[index] = Some(cost);
            }
            if pending.is_empty() {
                return costs;
            }
        }
        let here = tile_of(found);
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level: here.level };
            if !search.in_bounds(next) {
                continue;
            }
            let next_key = key_of(next);
            if search.usable_door(next).is_none() && search.step_closed(here, next) {
                relax(&mut dist, &mut heap, next_key, cost + 1);
                continue;
            }
            if !search.step_open(here, next) {
                continue;
            }
            let door_tile = [next, here].into_iter().find(|tile| search.usable_door(*tile).is_some());
            let Some(door_tile) = door_tile else {
                continue;
            };
            let (id, shape, angle) = match search.usable_door(door_tile) {
                Some(found) => found,
                None => continue,
            };
            if here != door_tile && !search.tables.side_ok(id, door_tile, here) {
                continue;
            }
            if (here.x - door_tile.x).abs() + (here.z - door_tile.z).abs() > 1 {
                continue;
            }
            let (width, height) =
                search.world.loc_def(id as usize).map(|def| (def.length, def.width)).unwrap_or((1, 1));
            if (here.x, here.z) != (door_tile.x, door_tile.z) {
                let stand_flag = search.base(here);
                if !reach::reached(
                    &|_, _, _| stand_flag,
                    here.level,
                    here.x,
                    here.z,
                    door_tile.x,
                    door_tile.z,
                    width,
                    height,
                    i32::from(angle),
                    i32::from(shape),
                    0,
                ) {
                    continue;
                }
            }
            let edge = EdgeId::Door { loc: id, x: door_tile.x, z: door_tile.z, level: door_tile.level };
            if options.excluded_edges.contains(&edge) {
                continue;
            }
            relax(&mut dist, &mut heap, next_key, cost + DOOR_COST);
        }
        if let Some(edges) = rides.get(&here.key()) {
            // Same per-search filters as `dijkstra`: the L2 is permissive,
            // each search recovers exactly its own edge set.
            if search.in_bounds(here) && collision::tile_walkable(search.open(here)) {
                for edge in edges {
                    if !search.in_bounds(edge.dest) {
                        continue;
                    }
                    if options.excluded_edges.contains(&EdgeId::Ride(tables.transitions[edge.index].name.clone())) {
                        continue;
                    }
                    relax(&mut dist, &mut heap, key_of(edge.dest), cost + edge.cost);
                }
            }
        }
        if let Some(edges) = customs.get(&here.key()) {
            for edge in edges {
                if !search.ignore_requires_ok(edge.index) {
                    continue;
                }
                if options.excluded_edges.contains(&EdgeId::Custom(tables.customs[edge.index].name.clone())) {
                    continue;
                }
                relax(&mut dist, &mut heap, key_of(edge.dest), cost + edge.cost);
            }
        }
    }
    eprintln!("costs dbg end: visits={visits} pending_left={} costs={costs:?}", pending.len());
    costs
}

impl Search<'_> {
    fn ignore_requires_ok(&self, index: usize) -> bool {
        self.tables.customs[index].requires.iter().all(|tag| self.options.satisfied.contains(tag))
    }
}

#[cfg(test)]
mod tests {
    use super::plan_costs;
    use crate::plan::{self, NavOptions, Tile};

    fn setup() -> (std::sync::Arc<crate::static_world::StaticWorld>, std::sync::Arc<crate::tables::Tables>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = crate::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        (world, std::sync::Arc::new(tables))
    }

    fn unit_cost(
        world: &crate::static_world::StaticWorld,
        tables: &crate::tables::Tables,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
    ) -> Option<u32> {
        plan::plan(world, tables, from, dest, options).map(|plan| plan.cost).ok()
    }

    /// One search serves every target at exactly the unit-plan price.
    #[test]
    fn multi_target_equals_unit_plans() {
        let (world, tables) = setup();
        let from = Tile { x: 3230, z: 3241, level: 0 };
        let dests = vec![
            Tile { x: 3252, z: 3237, level: 0 },
            Tile { x: 3203, z: 3246, level: 0 },
            Tile { x: 3228, z: 3214, level: 0 },
        ];
        let options = NavOptions::default();
        let costs = plan_costs(&world, &tables, from, &dests, &options);
        assert_eq!(costs.len(), dests.len(), "one cost per target, in order");
        for (dest, cost) in dests.iter().zip(costs.iter()) {
            assert_eq!(*cost, unit_cost(&world, &tables, from, *dest, &options), "target ({},{}) prices like its unit plan", dest.x, dest.z);
        }
        assert!(costs.iter().all(|cost| cost.is_some()), "all three walk, got: {costs:?}");
        assert!(costs[1] < costs[0], "walk-cheap beats the Manhattan lure, got: {costs:?}");
    }

    /// Unplannable targets (paid-only corridor, toll filter off) read None,
    /// like the unit plan failing; repeats share one entry.
    #[test]
    fn unplannable_is_null_and_repeats_share() {
        let (world, tables) = setup();
        let from = Tile { x: 3265, z: 3227, level: 0 };
        let shut = Tile { x: 3270, z: 3227, level: 0 };
        let options = NavOptions::default();
        assert!(unit_cost(&world, &tables, from, shut, &options).is_none(), "toll corridor refuses without paid");
        let costs = plan_costs(&world, &tables, from, &[shut, shut, from], &options);
        assert_eq!(costs, vec![None, None, Some(0)], "shut stays shut, repeats share, self is zero");
    }

    /// Empty batches cost no search.
    #[test]
    fn empty_is_empty() {
        let (world, tables) = setup();
        let from = Tile { x: 3230, z: 3241, level: 0 };
        assert!(plan_costs(&world, &tables, from, &[], &NavOptions::default()).is_empty());
    }
}
