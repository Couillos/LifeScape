#![forbid(unsafe_code)]
//! Dense per-search tables (lot N3, 2026-11-24).
//!
//! `dijkstra` and `plan_costs_capped` kept their distances (and the
//! planner its parents) in hash maps that grow from zero, one entry per
//! visited tile: up to 500 000 inserts and probes per search. A search
//! touches a few 64 x 64 squares, so the same tables become one array of
//! 4 096 cells per touched square and level, found through a small map
//! (one probe per block change, a one-block cache in front of it).
//!
//! The values stored and the order they are read in do not change: the
//! heap, the tie-break on tile keys and every cost are the planner's own.

use std::cell::Cell;

use crate::plan::{unkey, PrevEdge, Tile, U64Map, FRESH};

const CELLS: usize = 64 * 64;

/// 64 x 64 blocks of `T`, allocated on first write.
pub(crate) struct DenseMap<T: Copy> {
    fill: T,
    index: U64Map<u32>,
    blocks: Vec<Box<[T; CELLS]>>,
    /// Last block looked up: (block key + 1, slot). Zero means none.
    last: Cell<(u64, u32)>,
}

fn block_key(tile: Tile) -> u64 {
    let level = (tile.level as u64) & 0xFF;
    let block_z = (((tile.z + 8192) >> 6) as u64) & 0x7FFF;
    let block_x = (((tile.x + 8192) >> 6) as u64) & 0x7FFF;
    (level << 30) | (block_z << 15) | block_x
}

fn cell_of(tile: Tile) -> usize {
    (((tile.z & 63) << 6) | (tile.x & 63)) as usize
}

impl<T: Copy> DenseMap<T> {
    pub(crate) fn new(fill: T) -> DenseMap<T> {
        DenseMap { fill, index: U64Map::default(), blocks: Vec::new(), last: Cell::new((0, 0)) }
    }

    fn slot_of(&self, key: u64) -> Option<usize> {
        let (last_key, last_slot) = self.last.get();
        if last_key == key + 1 {
            return Some(last_slot as usize);
        }
        let slot = *self.index.get(&key)?;
        self.last.set((key + 1, slot));
        Some(slot as usize)
    }

    pub(crate) fn get(&self, tile: Tile) -> T {
        match self.slot_of(block_key(tile)) {
            Some(slot) => self.blocks[slot][cell_of(tile)],
            None => self.fill,
        }
    }

    pub(crate) fn set(&mut self, tile: Tile, value: T) {
        let key = block_key(tile);
        let slot = match self.slot_of(key) {
            Some(slot) => slot,
            None => {
                let slot = self.blocks.len();
                self.blocks.push(Box::new([self.fill; CELLS]));
                self.index.insert(key, slot as u32);
                self.last.set((key + 1, slot as u32));
                slot
            }
        };
        self.blocks[slot][cell_of(tile)] = value;
    }

    /// Blocks touched so far (tests).
    #[cfg(test)]
    pub(crate) fn blocks(&self) -> usize {
        self.blocks.len()
    }
}

impl DenseMap<u32> {
    /// The cost stored for a tile, `None` while unset.
    pub(crate) fn cost(&self, tile: Tile) -> Option<u32> {
        match self.get(tile) {
            u32::MAX => None,
            cost => Some(cost),
        }
    }
}

/// Best known cost per search state: plain tiles dense, opened door
/// tiles (the FRESH mark, rare) in a small map.
pub(crate) struct Dist {
    plain: DenseMap<u32>,
    fresh: U64Map<u32>,
}

impl Dist {
    pub(crate) fn new() -> Dist {
        Dist { plain: DenseMap::new(u32::MAX), fresh: U64Map::default() }
    }

    pub(crate) fn get(&self, key: u64) -> Option<u32> {
        if key & FRESH != 0 {
            return self.fresh.get(&key).copied();
        }
        self.plain.cost(unkey(key))
    }

    pub(crate) fn set(&mut self, key: u64, cost: u32) {
        if key & FRESH != 0 {
            self.fresh.insert(key, cost);
        } else {
            self.plain.set(unkey(key), cost);
        }
    }
}

/// The eight walk steps, in the order the search tries them.
pub(crate) const STEPS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

const UNSET: u8 = 0;
const ANNEX: u8 = 255;

/// Parent of each reached state. A plain walk step (parent a neighbour,
/// plain or opened) costs one byte: the step taken, plus 8 when the
/// parent is an opened state. Everything else (door clicks, rides,
/// customs, opened children) goes to a side table with the full record.
pub(crate) struct Parents {
    walk: DenseMap<u8>,
    annex: U64Map<(u64, PrevEdge)>,
}

impl Parents {
    pub(crate) fn new() -> Parents {
        Parents { walk: DenseMap::new(UNSET), annex: U64Map::default() }
    }

    pub(crate) fn set(&mut self, from_key: u64, next_key: u64, edge: PrevEdge) {
        if let (PrevEdge::Walk, 0) = (&edge, next_key & FRESH) {
            let (here, next) = (unkey(from_key), unkey(next_key));
            let step = (next.x - here.x, next.z - here.z);
            if here.level == next.level {
                if let Some(index) = STEPS.iter().position(|candidate| *candidate == step) {
                    let fresh_parent = if from_key & FRESH != 0 { 8 } else { 0 };
                    self.walk.set(next, 1 + index as u8 + fresh_parent);
                    return;
                }
            }
        }
        if next_key & FRESH == 0 {
            self.walk.set(unkey(next_key), ANNEX);
        }
        self.annex.insert(next_key, (from_key, edge));
    }

    /// Parent key and edge of a reached state, `None` for the start and
    /// for states never reached.
    pub(crate) fn get(&self, key: u64) -> Option<(u64, PrevEdge)> {
        if key & FRESH == 0 {
            match self.walk.get(unkey(key)) {
                UNSET => return None,
                ANNEX => {}
                code => {
                    let index = usize::from((code - 1) % 8);
                    let fresh_parent = code > 8;
                    let next = unkey(key);
                    let (step_x, step_z) = STEPS[index];
                    let parent = Tile { x: next.x - step_x, z: next.z - step_z, level: next.level };
                    let parent_key = parent.key() | if fresh_parent { FRESH } else { 0 };
                    return Some((parent_key, PrevEdge::Walk));
                }
            }
        }
        self.annex.get(&key).map(|(parent, edge)| (*parent, edge.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: i32, z: i32, level: i32) -> Tile {
        Tile { x, z, level }
    }

    #[test]
    fn dense_map_reads_what_it_wrote_across_blocks() {
        let mut map = DenseMap::new(u32::MAX);
        let tiles = [tile(3200, 3200, 0), tile(3199, 3200, 0), tile(3200, 3199, 0), tile(3200, 3200, 1), tile(-5, 7, 0), tile(100_000 % 16000, 9000, 2)];
        for (index, at) in tiles.iter().enumerate() {
            assert_eq!(map.get(*at), u32::MAX);
            map.set(*at, index as u32);
        }
        for (index, at) in tiles.iter().enumerate() {
            assert_eq!(map.get(*at), index as u32, "{at:?}");
        }
        assert_eq!(map.get(tile(3201, 3200, 0)), u32::MAX);
        assert!(map.blocks() >= 5);
    }

    #[test]
    fn dist_keeps_opened_states_apart_from_their_tile() {
        let mut dist = Dist::new();
        let at = tile(3000, 3100, 0);
        dist.set(at.key(), 7);
        dist.set(at.key() | FRESH, 12);
        assert_eq!(dist.get(at.key()), Some(7));
        assert_eq!(dist.get(at.key() | FRESH), Some(12));
        assert_eq!(dist.get(tile(3001, 3100, 0).key()), None);
    }

    #[test]
    fn parents_round_trip_walk_steps_and_side_records() {
        let mut parents = Parents::new();
        let here = tile(3200, 3200, 0);
        for (dx, dz) in STEPS {
            let next = tile(here.x + dx, here.z + dz, 0);
            parents.set(here.key(), next.key(), PrevEdge::Walk);
            let (parent, edge) = parents.get(next.key()).expect("walk parent");
            assert_eq!(parent, here.key());
            assert!(matches!(edge, PrevEdge::Walk));
            // From an opened parent the mark comes back.
            let after = tile(next.x + 1, next.z, 0);
            parents.set(next.key() | FRESH, after.key(), PrevEdge::Walk);
            assert_eq!(parents.get(after.key()).unwrap().0, next.key() | FRESH);
        }
        // A ride landing far away, then a better walk onto the same tile.
        let landing = tile(3205, 3200, 1);
        parents.set(here.key(), landing.key(), PrevEdge::Ride { index: 3, dest: landing, pre_open: None });
        assert!(matches!(parents.get(landing.key()).unwrap().1, PrevEdge::Ride { index: 3, .. }));
        let below = tile(3204, 3200, 1);
        parents.set(below.key(), landing.key(), PrevEdge::Walk);
        let (parent, edge) = parents.get(landing.key()).unwrap();
        assert_eq!(parent, below.key());
        assert!(matches!(edge, PrevEdge::Walk));
        // An opened child always lives in the side table.
        parents.set(here.key(), landing.key() | FRESH, PrevEdge::Walk);
        assert_eq!(parents.get(landing.key() | FRESH).unwrap().0, here.key());
        assert!(parents.get(tile(1, 1, 0).key()).is_none());
    }
}
