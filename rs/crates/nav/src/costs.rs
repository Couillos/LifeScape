#![forbid(unsafe_code)]
//! Multi-target walking costs (PRD 2, N1): one Dijkstra from the bot,
//! settled per target, stopped when every target is reached.
//!
//! Same edge rules as `plan.rs` (`dijkstra`, fully-filtered leg D):
//! walk steps read the usable-door view, doors need an open op, their
//! requirement tags and a reach-valid stand, rides ride the shared L2
//! with the same per-search filters. A settled pop is optimal by
//! Dijkstra, so each `Some(cost)` prices exactly what a unit plan would
//! (`multi_target_equals_unit_plans` pins it); blocked destination tiles
//! (tree, counter) cost their closest reachable tile, exactly like the
//! adjusted unit plan. Only truly disconnected targets read `None`.
//!
//! Known limit: a blocked destination tile (tree, counter) never settles
//! and reads `None`, while `plan()` would serve an adjusted plan to the
//! closest reachable tile, exactly like the adjusted unit plan. Pickers
//! keep ranking blocked spots behind planned instead of dropping to
//! Manhattan order.
//!
//! No wall-clock budget here (only the visit bound): batches settle
//! nearby targets in milliseconds, and a clock cut would read `None`
//! for farther targets in slow builds. The visit bound alone guarantees
//! termination; the op runs inline on the acceptor like `op_debug`.
//!
//! Cost horizon (CPU 2026-11-14): the search stops past `horizon(from,
//! dests)` = `COSTS_HORIZON_FLOOR + COSTS_HORIZON_FACTOR` x the farthest
//! target's Chebyshev distance. A target only reachable by a longer
//! detour (an NPC across a river, a ring tile behind a fence the pocket
//! flood cannot close) reads `None`, like an unplannable one: callers
//! rank nearby stands and spots, and such a target used to sweep the
//! whole reachable world (up to `MAX_VISITS`, 1.8 s live) for an answer
//! nobody picks.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::bfs::can_step;
use crate::collision::{self, leaf_step_extra, CollisionMap};
use crate::plan::{EdgeId, NavOptions, SharedEdges, Tile, U64Map, DOOR_COST, TRANSITION_COST};
use crate::reach;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Search budget: explored tiles before the area reads disconnected,
/// same bound as `plan.rs` (an exhaustive miss stops here, never hangs).
const MAX_VISITS: usize = 500_000;
/// Cost horizon floor and factor (module doc).
pub const COSTS_HORIZON_FLOOR: u32 = 64;
pub const COSTS_HORIZON_FACTOR: u32 = 4;

/// Costliest walk the batch still prices (module doc).
pub fn horizon(from: Tile, dests: &[Tile]) -> u32 {
    let farthest = dests.iter().map(|dest| (dest.x - from.x).abs().max((dest.z - from.z).abs())).max().unwrap_or(0);
    COSTS_HORIZON_FLOOR + COSTS_HORIZON_FACTOR * farthest.max(0) as u32
}

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
    // Tile-key maps with the planner's identity hash (CPU 2026-11-14:
    // SipHash over (x, z, level) tuples, and the world lock per F2P read,
    // were most of a batch once the danger map stopped dominating).
    base_flags: U64Map<u32>,
    open_flags: U64Map<u32>,
    doors: U64Map<Option<(u16, u8, u8)>>,
    f2p_zones: U64Map<bool>,
}

impl Search<'_> {
    fn in_bounds(&mut self, tile: Tile) -> bool {
        if !self.options.f2p_only {
            return true;
        }
        let zone = Tile { x: tile.x >> 3, z: tile.z >> 3, level: 0 }.key();
        if let Some(inside) = self.f2p_zones.get(&zone) {
            return *inside;
        }
        let inside = self.world.in_f2p(tile.x, tile.z);
        self.f2p_zones.insert(zone, inside);
        inside
    }

    fn base(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.base_flags.get(&tile.key()) {
            return *flags;
        }
        let flags = self.world.get(tile.x, tile.z, tile.level);
        self.base_flags.insert(tile.key(), flags);
        flags
    }

    fn door_at(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        if let Some(found) = self.doors.get(&tile.key()) {
            return *found;
        }
        let found = self.world.door_at(tile.x, tile.z, tile.level);
        self.doors.insert(tile.key(), found);
        found
    }

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
        if !self.tables.missing(id, tile, &self.options.satisfied).is_empty() {
            return None;
        }
        Some((id, shape, angle))
    }

    fn open(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.open_flags.get(&tile.key()) {
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
        self.open_flags.insert(tile.key(), flags);
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

/// Walkable targets still to settle, each with the least price of a path
/// whose last move is a ride or custom edge: `TRANSITION_COST` plus the
/// walk from the nearest landing on the target's level (`u32::MAX` when
/// none lands there).
fn goal_floors(shared: &SharedEdges, dests: &[Tile], settle: &U64Map<Vec<usize>>) -> Vec<(Tile, u32)> {
    let mut targets: Vec<Tile> = dests.iter().copied().filter(|dest| settle.contains_key(&dest.key())).collect();
    targets.sort_by_key(|tile| tile.key());
    targets.dedup();
    if targets.is_empty() {
        return Vec::new();
    }
    let (rides, customs) = shared;
    let landings = rides.values().flatten().map(|edge| edge.dest).chain(customs.values().flatten().map(|edge| edge.dest));
    let mut floors = vec![u32::MAX; targets.len()];
    for landing in landings {
        for (slot, target) in targets.iter().enumerate() {
            if landing.level == target.level {
                floors[slot] = floors[slot].min(TRANSITION_COST + landing.chebyshev(*target) as u32);
            }
        }
    }
    targets.into_iter().zip(floors).collect()
}

/// Lower bound of the price from `here` to the nearest pending target.
fn goal_bound(here: Tile, pending: &[(Tile, u32)]) -> u32 {
    pending
        .iter()
        .map(|(target, ride_floor)| {
            let walk = if here.level == target.level { here.chebyshev(*target) as u32 } else { u32::MAX };
            walk.min(*ride_floor)
        })
        .min()
        .unwrap_or(0)
}

fn relax(
    dist: &mut U64Map<u32>,
    dist_open: &mut U64Map<u32>,
    heap: &mut BinaryHeap<(Reverse<u32>, Reverse<Key>, bool)>,
    next: Key,
    cost: u32,
    opened: bool,
) {
    let map = if opened { dist_open } else { dist };
    let slot = tile_of(next).key();
    if map.get(&slot).is_some_and(|known| *known <= cost) {
        return;
    }
    map.insert(slot, cost);
    heap.push((Reverse(cost), Reverse(next), opened));
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
    plan_costs_with(world, tables, from, dests, options, &shared, None)
}

/// Same search on a precomputed L2 edge set (the service pool path).
pub(crate) fn plan_costs_with(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dests: &[Tile],
    options: &NavOptions,
    shared: &SharedEdges,
    memo: Option<&crate::pocket::PocketMemo>,
) -> Vec<Option<u32>> {
    plan_costs_capped(world, tables, from, dests, options, shared, memo, u32::MAX)
}

/// `plan_costs_with` whose horizon never exceeds `cap`: a target priced
/// past it reads `None` (the offline bank table, `site_banks`, prices
/// only routes that can still win).
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_costs_capped(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dests: &[Tile],
    options: &NavOptions,
    shared: &SharedEdges,
    memo: Option<&crate::pocket::PocketMemo>,
    cap: u32,
) -> Vec<Option<u32>> {
    let mut search = Search {
        world,
        tables,
        options,
        base_flags: U64Map::default(),
        open_flags: U64Map::default(),
        doors: U64Map::default(),
        f2p_zones: U64Map::default(),
    };
    let danger = crate::plan::DangerMap::new(&options.danger);
    let (rides, customs) = shared;
    // Walkable targets settle on pop (first pop is optimal by Dijkstra).
    let mut settle: U64Map<Vec<usize>> = U64Map::default();
    // Blocked targets never settle: the search tracks their closest
    // reachable tile with the same rank `dijkstra` uses, so the served
    // cost equals the adjusted unit plan, tile for tile.
    struct Blocked {
        dest: Tile,
        indices: Vec<usize>,
        best: Tile,
        rank: (i32, i32, bool, u32),
        /// Best prefix any tile can reach (`fallback::rank_floor`).
        floor: Option<(i32, i32, bool)>,
    }
    let mut blocked: Vec<Blocked> = Vec::new();
    for (index, dest) in dests.iter().enumerate() {
        if collision::tile_walkable(search.open(*dest)) {
            // A sealed pocket never settles: left `None` at once instead
            // of exhausting the search for every other target.
            if crate::pocket::sealed_with(memo, world, tables, shared, from, *dest) {
                continue;
            }
            settle.entry(dest.key()).or_default().push(index);
        } else if let Some(known) = blocked.iter_mut().find(|known| known.dest == *dest) {
            known.indices.push(index);
        } else {
            let walled = crate::fallback::walled_off(&mut |tile| search.open(tile), from, *dest);
            let rank = from.fallback_rank(*dest, 0, walled);
            let floor = crate::fallback::rank_floor(
                &mut |tile| if search.in_bounds(tile) { Some(search.open(tile)) } else { None },
                *dest,
            );
            blocked.push(Blocked { dest: *dest, indices: vec![index], best: from, rank, floor });
        }
    }
    let mut costs: Vec<Option<u32>> = vec![None; dests.len()];
    let mut dist: U64Map<u32> = U64Map::default();
    // Tiles reached through a door click (door stands open), apart from
    // the cheap walked-onto shut tiles (same rule as `dijkstra`: shut
    // wall doors never open from their own tile, the click needs a real
    // stand). No prev map here: costs never rebuild a chain.
    let mut dist_open: U64Map<u32> = U64Map::default();
    let mut heap: BinaryHeap<(Reverse<u32>, Reverse<Key>, bool)> = BinaryHeap::new();
    dist.insert(from.key(), 0);
    heap.push((Reverse(0), Reverse(key_of(from)), false));
    let mut visits = 0usize;
    let horizon = horizon(from, dests).min(cap);
    let mut pending = goal_floors(shared, dests, &settle);
    while let Some((Reverse(cost), Reverse(found), opened)) = heap.pop() {
        let known = if opened { &dist_open } else { &dist };
        if known.get(&tile_of(found).key()).is_some_and(|known| *known != cost) {
            continue;
        }
        visits += 1;
        if visits > MAX_VISITS || cost > horizon {
            break;
        }
        if let Some(indices) = settle.remove(&tile_of(found).key()) {
            for index in indices {
                costs[index] = Some(cost);
            }
            if settle.is_empty() && blocked.is_empty() {
                return costs;
            }
            pending.retain(|(target, _)| key_of(*target) != found);
        }
        let here = tile_of(found);
        // Goal bound (CPU 2026-11-14): with walkable targets only, a tile
        // that cannot reach any of them within the horizon is not
        // expanded. Every step costs >= 1 and moves <= 1 tile, every
        // ride/custom edge costs >= `TRANSITION_COST`, so the bound is a
        // lower bound: no target priced <= horizon loses its optimal
        // path, a target past it reads `None` as before. Before, a target
        // behind a fence swept the whole horizon disk around the start.
        if blocked.is_empty() && cost.saturating_add(goal_bound(here, &pending)) > horizon {
            continue;
        }
        let mut finalized = false;
        for target in blocked.iter_mut() {
            let walled = crate::fallback::walled_off(&mut |tile| search.open(tile), here, target.dest);
            let rank = here.fallback_rank(target.dest, cost, walled);
            if rank < target.rank {
                target.rank = rank;
                target.best = here;
            }
            // Rank floor popped in cost order: final (same rule as
            // `dijkstra`), priced now, the search goes on only for the
            // other targets.
            let prefix = (target.rank.0, target.rank.1, target.rank.2);
            if target.best == here && !opened && target.floor == Some(prefix) {
                for index in &target.indices {
                    costs[*index] = Some(cost);
                }
                target.indices.clear();
                finalized = true;
            }
        }
        if finalized {
            blocked.retain(|target| !target.indices.is_empty());
            if settle.is_empty() && blocked.is_empty() {
                return costs;
            }
        }
        // Walk and door neighbours, eight directions in fixed order.
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level: here.level };
            // Blocked destinations: tiles far past the current best can
            // never improve a fallback (same prune as `dijkstra`). Never
            // prunes while a walkable target is unsettled: it might lie
            // beyond.
            if settle.is_empty() && blocked.iter().all(|target| target.floor.is_none() && crate::fallback::past_fallback(next, target.dest, crate::fallback::prune_reach(from, target.dest, target.rank.1))) {
                continue;
            }
            if !search.in_bounds(next) {
                continue;
            }
            let next_key = key_of(next);
            // A step off a door tile, or anywhere no usable door stands,
            // walks when the shut view allows it (same rule as `dijkstra`).
            if search.usable_door(next).is_none() && search.step_closed(here, next) {
                relax(&mut dist, &mut dist_open, &mut heap, next_key, cost + 1 + danger.extra(next) + leaf_step_extra(search.base(here), search.base(next), dx, dz), false);
                continue;
            }
            // Stop onto a usable door tile from a table-allowed side,
            // walking only: the click edge rides along below when this
            // side is a real stand (same rule as `dijkstra`).
            if search.usable_door(here).is_none() {
                if let Some((id, _, _)) = search.usable_door(next) {
                    if search.tables.side_ok(id, next, here) && search.step_closed(here, next) {
                        relax(&mut dist, &mut dist_open, &mut heap, next_key, cost + 1 + danger.extra(next) + leaf_step_extra(search.base(here), search.base(next), dx, dz), false);
                    }
                }
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
            // A step off the door tile itself only continues an opening
            // the click made (open state: arrived through this door's
            // edge). Same rule as `dijkstra`: a shut wall door never
            // opens from its own tile, the click needs a real stand.
            if here == door_tile && matches!(shape, 0 | 1 | 2 | 3 | 9) && !opened {
                continue;
            }
            // Clickable stand only: the engine takes the op from an
            // orthogonal neighbour passing its reach rule for the door's
            // shape and angle (a diagonal or wrong-sided stand reads
            // "I can't reach that!" live). Stepping off the door tile
            // itself (stand == door) is the same opening's second half:
            // the click already happened from the previous stand, which
            // passed this check when stepping on. Flags are the closed
            // view: every nearby door is still shut at click time.
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
            relax(&mut dist, &mut dist_open, &mut heap, next_key, cost + DOOR_COST, false);
            // A real click opens the tile: same open state as `dijkstra`.
            if here != door_tile && next == door_tile {
                relax(&mut dist, &mut dist_open, &mut heap, next_key, cost + DOOR_COST, true);
            }
        }
        // Transition and custom rides from this stand. The shared L2 is
        // the permissive superset (requires/tolls/F2P lifted), so a stand
        // this search could not use, or a dest outside its bounds, is
        // skipped here: each search sees exactly the edges its own
        // prepare_edges would have built.
        if let Some(edges) = rides.get(&here.key()) {
            if search.in_bounds(here) && collision::tile_walkable(search.open(here)) {
                for edge in edges {
                    if !search.in_bounds(edge.dest) {
                        continue;
                    }
                    if options.excluded_edges.contains(&EdgeId::Ride(tables.transitions[edge.index].name.clone())) {
                        continue;
                    }
                    relax(&mut dist, &mut dist_open, &mut heap, key_of(edge.dest), cost + edge.cost, false);
                }
            }
        }
        if let Some(edges) = customs.get(&here.key()) {
            for edge in edges {
                // A fare edge (the Karamja boat) is gated by its coins
                // `requires` only, like `plan::dijkstra` (never `allow_paid`).
                if !search.tables.customs[edge.index].requires.iter().all(|tag| search.options.satisfied.contains(tag)) {
                    continue;
                }
                if options.excluded_edges.contains(&EdgeId::Custom(tables.customs[edge.index].name.clone())) {
                    continue;
                }
                relax(&mut dist, &mut dist_open, &mut heap, key_of(edge.dest), cost + edge.cost, false);
            }
        }
    }
    for target in &blocked {
        let best = dist.get(&target.best.key()).copied();
        for index in &target.indices {
            costs[*index] = best;
        }
    }
    costs
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
        world.set_npc_blockers(tables.npc_blockers.clone());
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

    /// Porte Peksa : le batch cote comme le plan unitaire, y compris la
    /// sortie par le stand est (pas de clic src==dest). Meme regle
    /// d'ouverture des deux cotes, jamais de divergence.
    #[test]
    fn peksa_exit_costs_match_the_unit_plan() {
        let (world, tables) = setup();
        let from = Tile { x: 3076, z: 3428, level: 0 };
        let dest = Tile { x: 3076, z: 3425, level: 0 };
        let options = NavOptions::default();
        let planned = plan::plan(&world, &tables, from, dest, &options).expect("la sortie Peksa planifie");
        assert!(
            planned.segments.iter().any(|segment| matches!(
                segment,
                plan::Segment::OpenDoor { door, stand, .. }
                if door.loc == 1530 && (stand.x, stand.z) == (3077, 3427)
            )),
            "le plan clique depuis l'est, got: {:?}",
            planned.segments,
        );
        let costs = plan_costs(&world, &tables, from, &[dest], &options);
        assert_eq!(costs, vec![Some(planned.cost)], "le batch cote comme le plan, got: {costs:?}");
    }

    /// Unplannable targets read None, like the failing unit plan; repeats
    /// share one entry and the source itself costs zero.
    #[test]
    fn unplannable_is_null_and_repeats_share() {
        let (world, tables) = setup();
        // Guild door without quest points: the unit plan refuses.
        let from = Tile { x: 3191, z: 3366, level: 0 };
        let shut = Tile { x: 3191, z: 3361, level: 0 };
        let options = NavOptions::default();
        assert!(unit_cost(&world, &tables, from, shut, &options).is_none(), "guild door refuses without qp");
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
