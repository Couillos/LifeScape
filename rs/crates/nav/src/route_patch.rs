#![forbid(unsafe_code)]
//! Local walk patch around one tile (route cache splices, CPU 2026-11-14).
//!
//! The splice legs are short walk-only searches within `SPLICE_RADIUS`
//! of the start or the destination. A `Patch` reads the world ONCE per
//! tile of its window (flags, door, F2P bound, danger) into flat arrays,
//! then runs both directions on them: forward (costs FROM the origin)
//! and reverse (costs TO the origin, each step priced as the forward
//! step it stands for). Same step rule and step price as the planner's
//! walk edge (`can_step` on the shut view, 1 + door-leaf extra); a tile
//! holding a door, outside the F2P bound or inside a danger square is
//! never entered, so a leg never needs a door click, a toll or a tag.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::bfs::can_step;
use crate::collision::leaf_step_extra;
use crate::plan::{NavOptions, Tile};

use crate::static_world::StaticWorld;

/// Flags + enterability of the window around one origin.
pub(crate) struct Patch {
    origin: Tile,
    radius: i32,
    /// Padded side (one extra ring: diagonal steps read the neighbours).
    padded: i32,
    flags: Vec<u32>,
    enterable: Vec<bool>,
}

/// Distances of one local search, origin excluded from chains.
pub(crate) struct LocalDist {
    origin: Tile,
    radius: i32,
    reverse: bool,
    dist: Vec<u32>,
    parent: Vec<u32>,
}

const UNREACHED: u32 = u32::MAX;
const STEPS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

impl Patch {
    pub fn read(world: &StaticWorld, options: &NavOptions, origin: Tile, radius: i32) -> Patch {
        let padded = 2 * radius + 3;
        let mut flags = Vec::with_capacity((padded * padded) as usize);
        let mut enterable = Vec::with_capacity((padded * padded) as usize);
        // One bitmap over the window instead of a scan of every square
        // per tile (same clamp, same answer: `danger.rs`).
        let danger = crate::plan::DangerMap::clipped(&options.danger, origin.x - radius, origin.z - radius, origin.x + radius, origin.z + radius);
        for offset_z in -radius - 1..=radius + 1 {
            for offset_x in -radius - 1..=radius + 1 {
                let tile = Tile { x: origin.x + offset_x, z: origin.z + offset_z, level: origin.level };
                flags.push(world.get(tile.x, tile.z, tile.level));
                let inside = offset_x.abs() <= radius && offset_z.abs() <= radius;
                enterable.push(
                    inside
                        && (!options.f2p_only || world.in_f2p(tile.x, tile.z))
                        && world.door_at(tile.x, tile.z, tile.level).is_none()
                        && !danger.contains(tile),
                );
            }
        }
        Patch { origin, radius, padded, flags, enterable }
    }

    fn slot(&self, x: i32, z: i32) -> Option<usize> {
        let (offset_x, offset_z) = (x - self.origin.x + self.radius + 1, z - self.origin.z + self.radius + 1);
        if offset_x < 0 || offset_z < 0 || offset_x >= self.padded || offset_z >= self.padded {
            return None;
        }
        Some((offset_z * self.padded + offset_x) as usize)
    }

    fn flag(&self, x: i32, z: i32) -> u32 {
        self.slot(x, z).map_or(u32::MAX, |slot| self.flags[slot])
    }

    /// The planner's walk edge `from -> to`, inside the window.
    fn walk(&self, from: Tile, to: Tile) -> Option<u32> {
        let slot = self.slot(to.x, to.z)?;
        if !self.enterable[slot] {
            return None;
        }
        let mut step = |x: i32, z: i32| self.flag(x, z);
        if !can_step(&mut step, from.x, from.z, to.x, to.z) {
            return None;
        }
        Some(1 + leaf_step_extra(self.flag(from.x, from.z), self.flag(to.x, to.z), to.x - from.x, to.z - from.z))
    }

    /// Bounded Dijkstra from the origin (`reverse`: costs TO it), nothing
    /// past `horizon`.
    pub fn search(&self, reverse: bool, horizon: u32) -> LocalDist {
        let size = self.flags.len();
        let mut dist = vec![UNREACHED; size];
        let mut parent = vec![UNREACHED; size];
        let mut heap: BinaryHeap<(Reverse<u32>, u32)> = BinaryHeap::new();
        let Some(start) = self.slot(self.origin.x, self.origin.z) else {
            return LocalDist { origin: self.origin, radius: self.radius, reverse, dist, parent };
        };
        dist[start] = 0;
        heap.push((Reverse(0), start as u32));
        while let Some((Reverse(cost), slot)) = heap.pop() {
            let slot = slot as usize;
            if cost != dist[slot] {
                continue;
            }
            let here = self.tile_of(slot);
            for (step_x, step_z) in STEPS {
                let next = Tile { x: here.x + step_x, z: here.z + step_z, level: here.level };
                let price = if reverse { self.walk_reverse(next, here) } else { self.walk(here, next) };
                let Some(price) = price else { continue };
                let total = cost + price;
                if total > horizon {
                    continue;
                }
                let Some(next_slot) = self.slot(next.x, next.z) else { continue };
                if total < dist[next_slot] {
                    dist[next_slot] = total;
                    parent[next_slot] = slot as u32;
                    heap.push((Reverse(total), next_slot as u32));
                }
            }
        }
        LocalDist { origin: self.origin, radius: self.radius, reverse, dist, parent }
    }

    /// Reverse edge: `from` steps onto `to` forward; `from` must be a
    /// tile the leg may stand on (inside, enterable) — the origin side
    /// is checked by the forward rule on `to`.
    fn walk_reverse(&self, from: Tile, to: Tile) -> Option<u32> {
        let slot = self.slot(from.x, from.z)?;
        if !self.enterable[slot] {
            return None;
        }
        self.walk(from, to)
    }

    fn tile_of(&self, slot: usize) -> Tile {
        let (offset_x, offset_z) = ((slot as i32) % self.padded, (slot as i32) / self.padded);
        Tile { x: self.origin.x + offset_x - self.radius - 1, z: self.origin.z + offset_z - self.radius - 1, level: self.origin.level }
    }
}

impl LocalDist {
    fn slot(&self, tile: Tile) -> Option<usize> {
        let padded = 2 * self.radius + 3;
        let (offset_x, offset_z) = (tile.x - self.origin.x + self.radius + 1, tile.z - self.origin.z + self.radius + 1);
        if tile.level != self.origin.level || offset_x < 0 || offset_z < 0 || offset_x >= padded || offset_z >= padded {
            return None;
        }
        Some((offset_z * padded + offset_x) as usize)
    }

    fn tile_of(&self, slot: usize) -> Tile {
        let padded = 2 * self.radius + 3;
        let (offset_x, offset_z) = ((slot as i32) % padded, (slot as i32) / padded);
        Tile { x: self.origin.x + offset_x - self.radius - 1, z: self.origin.z + offset_z - self.radius - 1, level: self.origin.level }
    }

    /// Settled cost of `tile`, if reached.
    pub fn get(&self, tile: Tile) -> Option<u32> {
        let slot = self.slot(tile)?;
        let cost = self.dist[slot];
        (cost != UNREACHED).then_some(cost)
    }

    /// Walking-order tiles of the leg through `tile` (forward search:
    /// origin -> tile, origin excluded; reverse search: tile -> origin,
    /// `tile` excluded, origin last). Empty when `tile` is the origin.
    pub fn leg(&self, tile: Tile) -> Vec<Tile> {
        let mut chain = Vec::new();
        let Some(mut slot) = self.slot(tile) else { return chain };
        while self.parent[slot] != UNREACHED {
            chain.push(self.tile_of(slot));
            slot = self.parent[slot] as usize;
        }
        if self.reverse {
            // chain = [tile, ..., last before origin]: walk order from the
            // tile toward the origin, minus the tile itself, plus origin.
            if chain.is_empty() {
                return chain;
            }
            chain.remove(0);
            chain.push(self.origin);
            chain
        } else {
            chain.reverse();
            chain
        }
    }
}
