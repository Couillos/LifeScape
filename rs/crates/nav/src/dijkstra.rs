#![forbid(unsafe_code)]
//! The planner's tile search (lot 5.7, 2026-11-24), split out of `plan.rs`.
//!
//! `dijkstra` was one 270-line function (cyclomatic ~60). It is now ONE
//! traversal (the loop below: pop the cheapest tile, expand it) made of
//! named parts, each reading like the rule it implements:
//!
//! - `StopRule`: when the traversal ends (budget, blocked-destination
//!   floor, destination popped);
//! - `Expander::walk_step`: the shut-view walk step, and the walk onto a
//!   door tile;
//! - `Expander::door_step`: click a door open (the `FRESH` twin state);
//! - `Expander::rides` / `Expander::customs`: transition and fare edges
//!   from the shared L2 set.
//!
//! The order of every read, comparison and `relax` is the one of the old
//! function: same pops, same ties, same plans byte for byte (replay
//! fingerprint, goldens).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

use crate::collision::{leaf_step_extra, tile_walkable};
use crate::dense::{Dist, Parents};
use crate::fallback::walled_off;
use crate::plan::{
    unkey, DangerMap, DoorRef, EdgeId, NavOptions, PrevEdge, SharedEdges, Tile, DOOR_COST, FRESH, MAX_VISITS,
};
use crate::reach;
use crate::search::Search;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

pub(crate) enum Outcome {
    Found(Vec<(Tile, PrevEdge)>, u32, Tile),
    Closest(Tile),
    /// Cut by `MAX_VISITS` or the wall deadline: the best tile so far.
    OverBudget(Tile),
    Aborted,
}

/// Search keepalive gate: the visit-count abort stays (operator stop
/// must bite even on a fast search), plus a wall-clock yield so a slow
/// search — live doors, toll detours — still proves the session alive
/// server-side instead of going quiet past the 100-tick reap.
pub(crate) fn yield_due(visits: usize, last_yield: Instant) -> bool {
    visits % 16384 == 0 || (visits % 16 == 0 && last_yield.elapsed() >= std::time::Duration::from_secs(1))
}

type Heap = BinaryHeap<(Reverse<u32>, Reverse<u64>)>;

/// Distances, parents and the queue of one search.
struct Frontier {
    dist: Dist,
    prev: Parents,
    heap: Heap,
}

impl Frontier {
    fn relax(&mut self, from_key: u64, next_key: u64, cost: u32, edge: PrevEdge) {
        if self.dist.get(next_key).is_some_and(|known| known <= cost) {
            return;
        }
        self.dist.set(next_key, cost);
        self.prev.set(from_key, next_key, edge);
        self.heap.push((Reverse(cost), Reverse(next_key)));
    }
}

fn rebuild(prev: &Parents, from: Tile, dest: Tile) -> Vec<(Tile, PrevEdge)> {
    // Full keys, FRESH mark included: an opened tile and its shut twin
    // are different search states with different parents (the open one
    // continues the click, the shut one the walk that reached the stand).
    // Recomputing the key from the tile would resurrect the shut path.
    let mut chain = Vec::new();
    let mut at_key = dest.key();
    while at_key != from.key() {
        let Some((parent_key, edge)) = prev.get(at_key) else {
            break;
        };
        chain.push((unkey(at_key), edge));
        at_key = parent_key;
    }
    chain.reverse();
    chain
}

/// The best tile seen so far by fallback rank (the answer when the
/// destination itself cannot be stood on, and the cut tile of a budget
/// stop).
struct Best {
    tile: Tile,
    rank: (i32, i32, bool, u32),
}

/// When the traversal stops. Reads only; the loop acts on the verdict.
struct StopRule {
    dest: Tile,
    dest_key: u64,
    dest_walkable: bool,
    /// Blocked destination: the best rank prefix any tile can reach.
    floor: Option<(i32, i32, bool)>,
    cost_cap: u32,
    deadline: Instant,
}

impl StopRule {
    /// Visit cap, cost cap or wall deadline (`no_route`): the search
    /// stops at its current best instead of burning the worker, as
    /// OverBudget — never a Closest (Disconnected) nor a blocked-dest
    /// adjustment to wherever the cut fell. The operator stop still wins
    /// before this check.
    fn over_budget(&self, visits: usize, cost: u32) -> bool {
        visits > MAX_VISITS || cost > self.cost_cap || (visits % 1024 == 0 && Instant::now() >= self.deadline)
    }

    /// Blocked destination (tree, booth, fishing spot): the best tile at
    /// the rank floor (`fallback::rank_floor`) is the best rank there is,
    /// and pops come in cost order, so nothing later can beat it. Same
    /// answer as the sweep, without exploring 64 tiles past it (CPU
    /// 2026-11-14: adjusted plans swept ~8-16k tiles each, a start
    /// already beside the target included).
    fn floor_reached(&self, here: Tile, fresh: bool, best: &Best) -> bool {
        best.tile == here && !fresh && self.floor == Some((best.rank.0, best.rank.1, best.rank.2))
    }

    /// The walkable destination was popped.
    fn dest_reached(&self, key: u64) -> bool {
        key == self.dest_key && self.dest_walkable
    }

    /// Blocked destination with no floor in reach: tiles far past the
    /// current best and the start are cut (`past_fallback`, `prune_reach`).
    /// With a floor the pop at its rank ends the search, so nothing is
    /// cut: a fence can make the only way to the floor walk 160 tiles off
    /// (live 2026-11-16, Al Kharid bank -> Lumbridge swamp adamantite
    /// round the north, admin "Disconnected" at the bank).
    fn pruned(&self, from: Tile, next: Tile, best_rank_distance: i32) -> bool {
        !self.dest_walkable
            && self.floor.is_none()
            && crate::fallback::past_fallback(next, self.dest, crate::fallback::prune_reach(from, self.dest, best_rank_distance))
    }
}

/// The edges out of one popped tile: walk, doors, rides, fares.
struct Expander<'a> {
    search: Search<'a>,
    options: &'a NavOptions,
    tables: &'a Tables,
    danger: DangerMap,
    shared: &'a SharedEdges,
    from: Tile,
    stop: StopRule,
    frontier: Frontier,
}

impl Expander<'_> {
    /// Walk and door neighbours, eight directions in fixed order.
    fn neighbours(&mut self, here: Tile, key: u64, cost: u32, best_rank_distance: i32) {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + dx, z: here.z + dz, level: here.level };
            if self.stop.pruned(self.from, next, best_rank_distance) {
                continue;
            }
            if !self.search.in_bounds(next) {
                continue;
            }
            // A step off a door tile, or anywhere no usable door stands,
            // walks when the shut view allows it: the opening (if any)
            // already rode the step onto the tile.
            if self.walk_step(here, key, cost, next, dx, dz) {
                continue;
            }
            self.walk_onto_door(here, key, cost, next, dx, dz);
            self.door_step(here, key, cost, next);
        }
    }

    fn step_price(&mut self, here: Tile, next: Tile, dx: i32, dz: i32, cost: u32) -> u32 {
        cost + 1 + self.danger.extra(next) + leaf_step_extra(self.search.base(here), self.search.base(next), dx, dz)
    }

    /// The plain walk: no usable door on `next` and the shut view lets
    /// the step through. True when taken (the neighbour is done).
    fn walk_step(&mut self, here: Tile, key: u64, cost: u32, next: Tile, dx: i32, dz: i32) -> bool {
        if self.search.usable_door(next).is_none() && self.search.step_closed(here, next) {
            let price = self.step_price(here, next, dx, dz, cost);
            self.frontier.relax(key, next.key(), price, PrevEdge::Walk);
            return true;
        }
        false
    }

    /// Stop onto a usable door tile from a side the table lets through,
    /// walking only (Peksa inside: the server holds the tile walkable,
    /// the walk reaches far-side stands like the east one). The click
    /// edge rides along in `door_step` when this side is a real stand:
    /// the search takes whichever wins, never a blind src==dest click.
    /// THROUGH still opens: walking off past the tile rides the door edge.
    fn walk_onto_door(&mut self, here: Tile, key: u64, cost: u32, next: Tile, dx: i32, dz: i32) {
        if self.search.usable_door(here).is_none() {
            if let Some((id, _, _)) = self.search.usable_door(next) {
                if self.search.tables.side_ok(id, next, here) && self.search.step_closed(here, next) {
                    let price = self.step_price(here, next, dx, dz, cost);
                    self.frontier.relax(key, next.key(), price, PrevEdge::Walk);
                }
            }
        }
    }

    /// The door edge: click a closed door open from a stand beside it (or
    /// from its own tile), then cross.
    fn door_step(&mut self, here: Tile, key: u64, cost: u32, next: Tile) {
        if !self.search.step_open(here, next) {
            return;
        }
        let door_tile = [next, here].into_iter().find(|tile| self.search.usable_door(*tile).is_some());
        let Some(door_tile) = door_tile else {
            return;
        };
        let (id, shape, angle) = match self.search.usable_door(door_tile) {
            Some(found) => found,
            None => return,
        };
        if here != door_tile && !self.search.tables.side_ok(id, door_tile, here) {
            return;
        }
        let fresh = key & FRESH != 0;
        // A click from the door tile itself: the engine reaches a
        // wall loc from its own tile (`ReachStrategy.reached` src==dest
        // arm) and `[oploc1,_door_closed]` checks no side (admin live
        // 2026-11-18: sewer 1530 (3241,9910) and Peksa 1530 open from
        // their tiles). A one-way door keeps its side rule for that
        // stand; a fresh state only continues the opening its click
        // already made from the previous stand.
        if here == door_tile && !fresh && !self.search.tables.side_ok(id, door_tile, here) {
            return;
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
            return;
        }
        let (width, height) =
            self.search.world.loc_def(id as usize).map(|def| (def.length, def.width)).unwrap_or((1, 1));
        if (here.x, here.z) != (door_tile.x, door_tile.z) {
            let stand_flag = self.search.base(here);
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
                return;
            }
        }
        let edge = EdgeId::Door { loc: id, x: door_tile.x, z: door_tile.z, level: door_tile.level };
        if self.options.excluded_edges.contains(&edge) {
            return;
        }
        let door = DoorRef {
            loc: id,
            x: door_tile.x,
            z: door_tile.z,
            level: door_tile.level,
            open_op: self.search.world.loc_def(id as usize).and_then(|def| def.open_op),
            toll: self.search.world.is_toll(door_tile.x, door_tile.z, door_tile.level),
        };
        let next_key = next.key();
        self.frontier.relax(key, next_key, cost + DOOR_COST, PrevEdge::Door(door.clone()));
        // A real click (stand beside the tile, stepping onto it)
        // opens the door: the tile is now passable open, a separate
        // search state from the cheap walked-onto shut tile (same
        // tile bits, FRESH mark). Only the open state steps off (see
        // above); the shut one keeps reaching far-side stands.
        if here != door_tile && next == door_tile {
            self.frontier.relax(key, next_key | FRESH, cost + DOOR_COST, PrevEdge::Door(door));
        }
    }

    /// Transition rides from this stand. The shared L2 is the permissive
    /// superset (requires/tolls/F2P lifted), so a stand this search could
    /// not use, or a dest outside its bounds, is skipped here: each
    /// search sees exactly the edges its own prepare_edges would have
    /// built. Fresh (opened) tiles ride from the same stands: the FRESH
    /// mark is search state, not a tile.
    fn rides(&mut self, here: Tile, key: u64, cost: u32) {
        let (rides, _) = self.shared;
        let l2_key = key & !FRESH;
        if let Some(edges) = rides.get(&l2_key) {
            if self.search.in_bounds(here) && tile_walkable(self.search.open(here)) {
                for edge in edges {
                    if !self.search.in_bounds(edge.dest) {
                        continue;
                    }
                    if self.options.excluded_edges.contains(&EdgeId::Ride(self.tables.transitions[edge.index].name.clone())) {
                        continue;
                    }
                    let ride = PrevEdge::Ride { index: edge.index, dest: edge.dest, pre_open: edge.pre_open.clone() };
                    self.frontier.relax(key, edge.dest.key(), cost + edge.cost, ride);
                }
            }
        }
    }

    /// Custom (fare) edges from this stand.
    fn customs(&mut self, key: u64, cost: u32) {
        let (_, customs) = self.shared;
        let l2_key = key & !FRESH;
        if let Some(edges) = customs.get(&l2_key) {
            for edge in edges {
                // A fare edge (the Karamja boat) is part of the map like any
                // door (user 2026-11-16: "aller à Karamja de manière
                // universelle"): its `requires` (the coins in the pack) gate
                // it, never `allow_paid` (the toll alone). Its landing obeys
                // the search bounds, like a ride's.
                if !self.search.in_bounds(edge.dest) {
                    continue;
                }
                if !self.search.ignore_requires
                    && !self.search.tables.customs[edge.index].requires.iter().all(|tag| self.search.options.satisfied.contains(tag))
                {
                    continue;
                }
                if self.options.excluded_edges.contains(&EdgeId::Custom(self.tables.customs[edge.index].name.clone())) {
                    continue;
                }
                let custom = PrevEdge::Custom { index: edge.index, dest: edge.dest };
                self.frontier.relax(key, edge.dest.key(), cost + edge.cost, custom);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dijkstra(
    world: &StaticWorld,
    tables: &Tables,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    ignore_requires: bool,
    allow_paid: bool,
    ignore_f2p: bool,
    shared: &SharedEdges,
    cost_cap: u32,
    yield_: &mut dyn FnMut() -> bool,
    deadline: Instant,
) -> Outcome {
    let mut search = Search::new(world, tables, options, ignore_requires, allow_paid, ignore_f2p);
    let dest_walkable = tile_walkable(search.open(dest));
    let floor = if dest_walkable {
        None
    } else {
        crate::fallback::rank_floor(&mut |tile| if search.in_bounds(tile) { Some(search.open(tile)) } else { None }, dest)
    };
    let mut best = Best {
        tile: from,
        rank: from.fallback_rank(dest, 0, walled_off(&mut |tile| search.open(tile), from, dest)),
    };
    let mut frontier = Frontier { dist: Dist::new(), prev: Parents::new(), heap: BinaryHeap::new() };
    frontier.dist.set(from.key(), 0);
    frontier.heap.push((Reverse(0), Reverse(from.key())));
    let mut walk = Expander {
        search,
        options,
        tables,
        danger: DangerMap::new(&options.danger),
        shared,
        from,
        stop: StopRule { dest, dest_key: dest.key(), dest_walkable, floor, cost_cap, deadline },
        frontier,
    };
    let mut visits = 0usize;
    // Wall-clock yield: a huge search (live doors, toll detours) can run
    // seconds without hitting the visit-count yield below, and the live
    // caller only emits its keepalive from the yield callback — a quiet
    // search reads as a dead session server-side (reaped after 100
    // ticks). Yield at most once a second, on top of the count check.
    let mut last_yield = Instant::now();
    let loop_start = Instant::now();
    let mut heap_max = 0usize;
    // Lowest cost first, lowest tile key breaking ties: the same world
    // always yields the same plan.
    while let Some((Reverse(cost), Reverse(key))) = walk.frontier.heap.pop() {
        if walk.frontier.dist.get(key).is_some_and(|known| known != cost) {
            continue;
        }
        visits += 1;
        if yield_due(visits, last_yield) {
            last_yield = Instant::now();
            if !yield_() {
                crate::nav_meter::add_visits(visits);
                return Outcome::Aborted;
            }
        }
        if walk.frontier.heap.len() > heap_max {
            heap_max = walk.frontier.heap.len();
        }
        if walk.stop.over_budget(visits, cost) {
            crate::nav_meter::add_visits(visits);
            crate::nav_meter::note_unreached(visits, true);
            return Outcome::OverBudget(best.tile);
        }
        let here = unkey(key);
        let fresh = key & FRESH != 0;
        let walled = walled_off(&mut |tile| walk.search.open(tile), here, dest);
        let rank = here.fallback_rank(dest, cost, walled);
        if rank < best.rank {
            best = Best { tile: here, rank };
        }
        if walk.stop.floor_reached(here, fresh, &best) {
            crate::nav_meter::add_visits(visits);
            return Outcome::Found(rebuild(&walk.frontier.prev, from, best.tile), cost, best.tile);
        }
        if walk.stop.dest_reached(key) {
            if std::env::var("PLAN_DEBUG").is_ok() {
                eprintln!("plan found visits={visits} heap_max={heap_max} {:?}", loop_start.elapsed());
            }
            crate::nav_meter::add_visits(visits);
            return Outcome::Found(rebuild(&walk.frontier.prev, from, dest), cost, dest);
        }
        walk.neighbours(here, key, cost, best.rank.1);
        walk.rides(here, key, cost);
        walk.customs(key, cost);
    }
    crate::nav_meter::add_visits(visits);
    // Blocked destination (tree, counter): the closest tile is the plan.
    if !dest_walkable && best.tile.key() != dest.key() && walk.frontier.dist.get(best.tile.key()).is_some() {
        let cost = walk.frontier.dist.get(best.tile.key()).expect("best was reached");
        if std::env::var("PLAN_DEBUG").is_ok() {
            eprintln!("plan adjusted visits={visits} {:?}", loop_start.elapsed());
        }
        return Outcome::Found(rebuild(&walk.frontier.prev, from, best.tile), cost, best.tile);
    }
    if std::env::var("PLAN_DEBUG").is_ok() {
        eprintln!("plan closest visits={visits} {:?}", loop_start.elapsed());
    }
    crate::nav_meter::note_unreached(visits, false);
    Outcome::Closest(best.tile)
}

#[cfg(test)]
mod tests {
    use super::yield_due;
    use std::time::{Duration, Instant};

    /// The keepalive gate: count abort every 16384 visits, wall-clock
    /// yield every 16 visits past one second, silence otherwise.
    #[test]
    fn yield_gate_count_and_clock() {
        let fresh = Instant::now();
        let old = Instant::now() - Duration::from_secs(2);
        assert!(yield_due(16384, fresh), "count abort bites even on a fast search");
        assert!(yield_due(2048, old), "slow search yields past one second");
        assert!(!yield_due(2048, fresh), "no yield mid-second");
        assert!(!yield_due(100, old), "no yield off the visit grid");
        assert!(!yield_due(17, old), "no yield off the 16-visit grid");
    }
}
