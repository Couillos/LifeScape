#![forbid(unsafe_code)]
//! Dense immutable grid behind the world reader (lot N2, 2026-11-24).
//!
//! One `Square` per 64 x 64 square and level, built once when first read
//! and never written again: the final collision flags (sky rule and
//! blocking NPCs applied, exactly what `StaticWorld::get_paged` answers),
//! one bit per tile for "a closed door stands here", one bit for "a door
//! stands within the 3 x 3 around" and one bit per 8 x 8 zone for free to
//! play ground. A read is two shifts, an index and a load: no lock, no
//! hashing. Squares sit in a flat array of `OnceLock<Arc<Square>>`
//! indexed by (level, square z, square x): built at the first demand,
//! read lock-free afterwards, no `unsafe`.
//!
//! The grid only caches. The paged reader it was built from
//! (`StaticWorld::get_paged`, `door_at_paged`, `in_f2p_paged`) stays in
//! place as the test oracle; a test compares every tile of every painted
//! square against it. The setters that change what a read answers
//! (`set_f2p`, `set_sky_floors`, `set_npc_blockers`) swap in an empty
//! grid. Tiles outside the indexed range (negative, or past 16 384, or
//! a level above 3) never enter the grid: callers fall back to the paged
//! reader.

use std::sync::{Arc, OnceLock};

use crate::static_world::StaticWorld;

/// Squares per axis in the flat array (tile coordinate 0..16 384).
const SQUARES_PER_AXIS: usize = 256;
const LEVELS: usize = 4;
const TILES: usize = 64 * 64;

/// One painted square at one level.
pub(crate) struct Square {
    flags: Box<[u32; TILES]>,
    /// A closed door loc is recorded on this exact tile.
    door: [u64; 64],
    /// A door lies on this tile or one of its 8 neighbours. Conservative
    /// on the square's border ring (the neighbour may lie in another
    /// square): set there, so the caller reads the exact doors.
    near_door: [u64; 64],
    /// Free-to-play bit per 8 x 8 zone (all set when no bounds exist).
    f2p: u64,
}

fn slot_of(tile_x: i32, tile_z: i32) -> usize {
    (((tile_z & 63) << 6) | (tile_x & 63)) as usize
}

fn bit(words: &[u64; 64], slot: usize) -> bool {
    words[slot >> 6] & (1u64 << (slot & 63)) != 0
}

fn set_bit(words: &mut [u64; 64], slot: usize) {
    words[slot >> 6] |= 1u64 << (slot & 63);
}

impl Square {
    /// Assemble a square from its final flags, its door tiles (slots)
    /// and its free-to-play zone bitmap.
    pub(crate) fn new(flags: Box<[u32; TILES]>, door_slots: &[usize], f2p: u64) -> Square {
        let mut door = [0u64; 64];
        for slot in door_slots {
            set_bit(&mut door, *slot);
        }
        let mut near_door = [0u64; 64];
        for z in 0..64usize {
            for x in 0..64usize {
                let border = x == 0 || z == 0 || x == 63 || z == 63;
                let mut near = border;
                if !border {
                    'scan: for dz in 0..3usize {
                        for dx in 0..3usize {
                            if bit(&door, ((z + dz - 1) << 6) | (x + dx - 1)) {
                                near = true;
                                break 'scan;
                            }
                        }
                    }
                }
                if near {
                    set_bit(&mut near_door, (z << 6) | x);
                }
            }
        }
        Square { flags, door, near_door, f2p }
    }

    pub(crate) fn flags(&self, tile_x: i32, tile_z: i32) -> u32 {
        self.flags[slot_of(tile_x, tile_z)]
    }

    pub(crate) fn has_door(&self, tile_x: i32, tile_z: i32) -> bool {
        bit(&self.door, slot_of(tile_x, tile_z))
    }

    pub(crate) fn near_door(&self, tile_x: i32, tile_z: i32) -> bool {
        bit(&self.near_door, slot_of(tile_x, tile_z))
    }

    pub(crate) fn in_f2p(&self, tile_x: i32, tile_z: i32) -> bool {
        let zone = (((tile_z >> 3) & 7) << 3) | ((tile_x >> 3) & 7);
        self.f2p & (1u64 << zone) != 0
    }
}

/// The flat array of squares of one world state.
pub(crate) struct Grid {
    slots: Vec<OnceLock<Arc<Square>>>,
}

impl Grid {
    pub(crate) fn new() -> Grid {
        Grid { slots: (0..LEVELS * SQUARES_PER_AXIS * SQUARES_PER_AXIS).map(|_| OnceLock::new()).collect() }
    }

    /// Slot of a tile, `None` outside the indexed range.
    fn index(tile_x: i32, tile_z: i32, level: i32) -> Option<usize> {
        if !(0..LEVELS as i32).contains(&level) || tile_x < 0 || tile_z < 0 {
            return None;
        }
        let (square_x, square_z) = ((tile_x >> 6) as usize, (tile_z >> 6) as usize);
        if square_x >= SQUARES_PER_AXIS || square_z >= SQUARES_PER_AXIS {
            return None;
        }
        Some((level as usize * SQUARES_PER_AXIS + square_z) * SQUARES_PER_AXIS + square_x)
    }

    /// The square holding this tile, built on first demand. `None` for a
    /// tile outside the range and when the pack square cannot be read
    /// (nothing is cached then: the paged reader answers and retries).
    pub(crate) fn square(&self, world: &StaticWorld, tile_x: i32, tile_z: i32, level: i32) -> Option<&Square> {
        let index = Grid::index(tile_x, tile_z, level)?;
        let slot = &self.slots[index];
        if let Some(found) = slot.get() {
            return Some(found);
        }
        let built = world.build_square(tile_x >> 6, tile_z >> 6, level)?;
        // A concurrent builder may have won: the squares are equal.
        let _ = slot.set(built);
        slot.get().map(|found| &**found)
    }

    /// Squares built so far (tests).
    #[cfg(test)]
    pub(crate) fn built(&self) -> usize {
        self.slots.iter().filter(|slot| slot.get().is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_covers_the_range_and_refuses_the_rest() {
        assert_eq!(Grid::index(0, 0, 0), Some(0));
        assert_eq!(Grid::index(63, 63, 0), Some(0));
        assert_eq!(Grid::index(64, 0, 0), Some(1));
        assert_eq!(Grid::index(0, 64, 0), Some(SQUARES_PER_AXIS));
        assert_eq!(Grid::index(0, 0, 1), Some(SQUARES_PER_AXIS * SQUARES_PER_AXIS));
        assert!(Grid::index(-1, 0, 0).is_none());
        assert!(Grid::index(0, -1, 0).is_none());
        assert!(Grid::index(0, 0, 4).is_none());
        assert!(Grid::index(0, 0, -1).is_none());
        assert!(Grid::index(16_384, 0, 0).is_none());
        assert!(Grid::index(16_383, 16_383, 3).is_some());
    }

    #[test]
    fn near_door_covers_the_3x3_and_the_border_ring() {
        let flags = Box::new([0u32; TILES]);
        let door = slot_of(10, 20);
        let square = Square::new(flags, &[door], u64::MAX);
        assert!(square.has_door(10, 20));
        assert!(!square.has_door(11, 20));
        for dz in -1..=1 {
            for dx in -1..=1 {
                assert!(square.near_door(10 + dx, 20 + dz), "{dx},{dz}");
            }
        }
        assert!(!square.near_door(12, 20));
        assert!(!square.near_door(10, 22));
        // Border ring: always "maybe".
        assert!(square.near_door(0, 5) && square.near_door(63, 5) && square.near_door(5, 0) && square.near_door(5, 63));
        assert!(!square.near_door(1, 5));
    }

    #[test]
    fn f2p_bit_per_zone() {
        let flags = Box::new([0u32; TILES]);
        let square = Square::new(flags, &[], 1u64 << ((2 << 3) | 5));
        assert!(square.in_f2p(5 * 8 + 3, 2 * 8 + 7));
        assert!(!square.in_f2p(4 * 8, 2 * 8));
        assert!(!square.in_f2p(5 * 8, 3 * 8));
    }
}
