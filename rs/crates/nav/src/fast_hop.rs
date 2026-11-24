#![forbid(unsafe_code)]
//! Short hops answered without the planner setup (CPU 2026-11-14).
//!
//! Live mix: 52 % of the plans are 3 tiles or less on one level (a bot
//! re-aligning on a chicken, a tree, a rock before each click), 77 % are
//! 8 or less, and 1 % ask for the tile the bot stands on. Each one used
//! to pay the whole planner: queue hand-off to the pool, sealed-pocket
//! flood of a new destination tile, `route_path::analyze`, route cache
//! insert. A short hop is a walk inside a few tiles: this module runs the
//! SAME Dijkstra (same step rule `bfs::can_step`, same step price `1 +
//! danger + door leaf`, same F2P bound, same (cost, tile key) pop order)
//! on a 33 x 33 window around the start, plain walk edges only, and
//! answers only when it can PROVE the full planner prices the hop the
//! same.
//!
//! Proof (`short_hop`). Let C be the cost at which the destination pops;
//! every tile popped before it costs <= C. In `plan::dijkstra`, the edges
//! from `here` to a neighbour `next` are exactly the plain walk edge
//! (static `can_step`, price above) whenever neither tile holds a usable
//! door: every other branch needs `usable_door(here)` or
//! `usable_door(next)` (door click, walk onto a door tile, `door_tile`
//! in {next, here}); the open view only matters inside those branches.
//! So the search expands `here` with plain walk edges and SKIPS: every
//! edge of a tile holding a door loc (any door: usable or not), the edge
//! onto a neighbour holding one, every edge of a tile on the window
//! border, and leaves rides/customs starting on `here`. A skipped planner
//! edge costs at least 1 (walk) or `TRANSITION_COST` (ride, custom), so a
//! path through it costs at least `bound` = cost(here) + that. If C <=
//! the least bound, no planner path is cheaper than C: it would leave the
//! window graph through a skipped edge (>= bound >= C) — the planner's
//! optimum is C. Otherwise the request goes to the full planner.
//! The destination must be walkable in the static view (a blocked one is
//! an adjusted plan, never answered here); the sealed-pocket check needs
//! no run: the walk found here (walkable tiles, static flags) is a path
//! of the pocket flood's permissive view, so the flood reaches the start.
//!
//! Plan shape = `plan::assemble` of a walk-only chain: one `Walk`
//! segment (none for a zero-distance hop), no adjusted destination, no
//! blocker. Pinned by goldens and a randomized equivalence against the
//! planner (`fast_hop_tests.rs`, live replay variant included).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::bfs::can_step;
use crate::collision::{leaf_step_extra, tile_walkable};
use crate::reader::Reader;
use crate::plan::{DangerMap, NavOptions, Plan, Segment, SharedEdges, Tile, TRANSITION_COST};
use crate::static_world::StaticWorld;

/// Largest request (Chebyshev, same level) the fast path tries.
pub const FAST_RADIUS: i32 = 8;
/// Search window half-side around the start: room for a detour twice
/// the request radius before the border stops the proof.
const WINDOW: i32 = 16;
/// Reads reach one tile past an expanded tile (neighbours, diagonals).
const MARGIN: i32 = 1;
/// Pop bound: the window holds 33 x 33 = 1089 tiles.
const MAX_POPS: usize = 1100;

const SIDE: i32 = 2 * (WINDOW + MARGIN) + 1;
const UNKNOWN: u32 = u32::MAX;

/// Why a short request was not answered here (counters, tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// Not a short same-level request: never tried.
    NotShort,
    /// Destination blocked in the static view (adjusted plan).
    BlockedDest,
    /// A door, ride or the window border could change the price.
    Unproven,
    /// Unreachable inside the window (the planner decides).
    NotFound,
}

/// Static reads of one window, each tile read once.
struct Window<'a> {
    reader: Reader<'a>,
    min_x: i32,
    min_z: i32,
    level: i32,
    flags: Vec<u32>,
    doors: Vec<u8>,
}

impl Window<'_> {
    fn slot(&self, tile_x: i32, tile_z: i32) -> usize {
        ((tile_z - self.min_z) * SIDE + (tile_x - self.min_x)) as usize
    }

    fn tile(&self, slot: usize) -> Tile {
        Tile { x: self.min_x + slot as i32 % SIDE, z: self.min_z + slot as i32 / SIDE, level: self.level }
    }

    fn flags(&mut self, tile_x: i32, tile_z: i32) -> u32 {
        let slot = self.slot(tile_x, tile_z);
        if self.flags[slot] == UNKNOWN {
            self.flags[slot] = self.reader.flags(Tile { x: tile_x, z: tile_z, level: self.level });
        }
        self.flags[slot]
    }

    /// Door loc on the tile (any door, usable or not: the proof only
    /// needs "none").
    fn door(&mut self, tile_x: i32, tile_z: i32) -> bool {
        let slot = self.slot(tile_x, tile_z);
        if self.doors[slot] == 0 {
            self.doors[slot] = if self.reader.door_at(Tile { x: tile_x, z: tile_z, level: self.level }).is_some() { 2 } else { 1 };
        }
        self.doors[slot] == 2
    }
}

/// Walk-only plan of a short hop, or why the full planner must answer.
pub(crate) fn short_hop(world: &StaticWorld, shared: &SharedEdges, from: Tile, dest: Tile, options: &NavOptions) -> Result<Plan, Decline> {
    if from.level != dest.level || from.chebyshev(dest) > FAST_RADIUS {
        return Err(Decline::NotShort);
    }
    let reader = Reader::new(world);
    if !tile_walkable(reader.flags(dest)) {
        return Err(Decline::BlockedDest);
    }
    if from == dest {
        // The planner pops the start first: found at cost 0, no segment
        // (no bound check on the start tile, like `dijkstra`).
        return Ok(Plan { segments: Vec::new(), cost: 0, adjusted_dest: None, dest_blocker: None });
    }
    let (min_x, min_z) = (from.x - WINDOW - MARGIN, from.z - WINDOW - MARGIN);
    let cells = (SIDE * SIDE) as usize;
    let mut window = Window { reader, min_x, min_z, level: from.level, flags: vec![UNKNOWN; cells], doors: vec![0; cells] };
    let danger = DangerMap::clipped(&options.danger, from.x - WINDOW, from.z - WINDOW, from.x + WINDOW, from.z + WINDOW);
    let (rides, customs) = shared;
    let mut dist = vec![u32::MAX; cells];
    let mut parent = vec![u32::MAX; cells];
    let mut heap: BinaryHeap<(Reverse<u32>, Reverse<u64>, u32)> = BinaryHeap::new();
    let start = window.slot(from.x, from.z);
    dist[start] = 0;
    heap.push((Reverse(0), Reverse(from.key()), start as u32));
    // Least price of a path through a skipped planner edge.
    let mut bound = u32::MAX;
    let mut pops = 0usize;
    while let Some((Reverse(cost), _, slot)) = heap.pop() {
        let slot = slot as usize;
        if dist[slot] != cost {
            continue;
        }
        if cost > bound {
            crate::nav_meter::add_visits(pops);
            return Err(Decline::Unproven);
        }
        pops += 1;
        if pops > MAX_POPS {
            crate::nav_meter::add_visits(pops);
            return Err(Decline::Unproven);
        }
        let here = window.tile(slot);
        if here == dest {
            crate::nav_meter::add_visits(pops);
            return walk_plan(&mut window, &parent, from, dest, cost).ok_or(Decline::Unproven);
        }
        if rides.contains_key(&here.key()) || customs.contains_key(&here.key()) {
            bound = bound.min(cost + TRANSITION_COST);
        }
        if here.chebyshev(from) >= WINDOW || window.door(here.x, here.z) {
            bound = bound.min(cost + 1);
            continue;
        }
        let here_flags = window.flags(here.x, here.z);
        for (step_x, step_z) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let next = Tile { x: here.x + step_x, z: here.z + step_z, level: here.level };
            if options.f2p_only && !window.reader.in_f2p(next) {
                continue;
            }
            if window.door(next.x, next.z) {
                bound = bound.min(cost + 1);
                continue;
            }
            let mut read = |tile_x: i32, tile_z: i32| window.flags(tile_x, tile_z);
            if !can_step(&mut read, here.x, here.z, next.x, next.z) {
                continue;
            }
            let next_flags = window.flags(next.x, next.z);
            let price = cost + 1 + danger.extra(next) + leaf_step_extra(here_flags, next_flags, step_x, step_z);
            let next_slot = window.slot(next.x, next.z);
            if dist[next_slot] <= price {
                continue;
            }
            dist[next_slot] = price;
            parent[next_slot] = slot as u32;
            heap.push((Reverse(price), Reverse(next.key()), next_slot as u32));
        }
    }
    crate::nav_meter::add_visits(pops);
    Err(Decline::NotFound)
}

/// The walked tiles, start excluded. `None` when a path tile is not
/// walkable in the static view: the sealed-pocket flood stops on such a
/// tile, so only the planner may answer (never seen: a step enters a
/// walkable tile).
fn walk_plan(window: &mut Window<'_>, parent: &[u32], from: Tile, dest: Tile, cost: u32) -> Option<Plan> {
    let mut tiles = Vec::new();
    let mut slot = window.slot(dest.x, dest.z);
    let start = window.slot(from.x, from.z);
    while slot != start {
        let tile = window.tile(slot);
        if !tile_walkable(window.flags(tile.x, tile.z)) {
            return None;
        }
        tiles.push(tile);
        slot = parent[slot] as usize;
    }
    tiles.reverse();
    Some(Plan { segments: vec![Segment::Walk { tiles }], cost, adjusted_dest: None, dest_blocker: None })
}
