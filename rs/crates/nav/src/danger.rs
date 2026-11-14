#![forbid(unsafe_code)]
//! Danger squares (`NavOptions::danger`) and their per-search lookup.
//!
//! CPU 2026-11-14: live workers send every aggressive spawn within 250
//! tiles (`kit::danger::arm`: 100-250 squares, radius up to ~17), and the
//! old `DangerMap` inserted every covered tile into a `HashSet` — 40-67 k
//! SipHash inserts, ~7 ms, built once per Dijkstra, once per
//! `route_path::analyze` and once per `walk_costs`: the largest fixed cost
//! of a plan by far (a 1-tile hop paid 14 ms of it). The map is now a
//! dense bitmap over the bounding box of the squares, per level (rows of
//! `u64` words, one range fill per square row): ~200 squares build in tens
//! of microseconds and a lookup is a bounds check plus one bit test.
//! Same answer for every tile (`bitmap_matches_tile_set` pins it against
//! the old set). A box too large for the bitmap (squares spread over the
//! whole map, never sent live) keeps the exact tile set.

use std::collections::HashSet;

use crate::plan::Tile;

/// One danger square: aggressive spawn plus its reach (Chebyshev).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DangerZone {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub radius: i32,
}

/// Extra cost of one walk step landing in a danger square: a detour of
/// up to this many tiles per avoided tile beats walking past a hunter.
pub const DANGER_STEP_COST: u32 = 8;

/// Radius clamp shared by every reader (map, cache checks).
pub(crate) const RADIUS_CLAMP: i32 = 32;

/// Largest bitmap per level (bits): 8 Mbit = 1 MiB, far above the
/// ~570 x 570 box of a live 250-tile arm.
const MAX_BITS: i64 = 1 << 23;

/// Covered tiles of one level as a dense bitmap over their box.
struct Layer {
    level: i32,
    min_x: i32,
    min_z: i32,
    width: i32,
    height: i32,
    row_words: usize,
    bits: Vec<u64>,
}

impl Layer {
    fn contains(&self, tile_x: i32, tile_z: i32) -> bool {
        let column = tile_x - self.min_x;
        let row = tile_z - self.min_z;
        if column < 0 || row < 0 || column >= self.width || row >= self.height {
            return false;
        }
        let word = row as usize * self.row_words + (column as usize >> 6);
        self.bits[word] & (1u64 << (column as usize & 63)) != 0
    }

    /// Sets columns `first..=last` of `row` (box-relative, in range).
    fn fill(&mut self, row: i32, first: i32, last: i32) {
        let base = row as usize * self.row_words;
        let (first, last) = (first as usize, last as usize);
        for word in first >> 6..=last >> 6 {
            let low = if word == first >> 6 { first & 63 } else { 0 };
            let high = if word == last >> 6 { last & 63 } else { 63 };
            let mask = (u64::MAX >> (63 - high)) & (u64::MAX << low);
            self.bits[base + word] |= mask;
        }
    }
}

/// Danger tiles of one search, built once per search (or per window).
#[derive(Default)]
pub struct DangerMap {
    layers: Vec<Layer>,
    /// Exact tile set for a box past `MAX_BITS` (never live).
    spread: Option<HashSet<(i32, i32, i32)>>,
}

/// Square bounds after the radius clamp, intersected with `clip`.
fn bounds(zone: &DangerZone, clip: Option<(i32, i32, i32, i32)>) -> Option<(i32, i32, i32, i32)> {
    let radius = zone.radius.clamp(0, RADIUS_CLAMP);
    let (mut min_x, mut min_z, mut max_x, mut max_z) = (zone.x - radius, zone.z - radius, zone.x + radius, zone.z + radius);
    if let Some((clip_min_x, clip_min_z, clip_max_x, clip_max_z)) = clip {
        min_x = min_x.max(clip_min_x);
        min_z = min_z.max(clip_min_z);
        max_x = max_x.min(clip_max_x);
        max_z = max_z.min(clip_max_z);
    }
    (min_x <= max_x && min_z <= max_z).then_some((min_x, min_z, max_x, max_z))
}

impl DangerMap {
    pub fn new(zones: &[DangerZone]) -> DangerMap {
        Self::build(zones, None)
    }

    /// Same map restricted to the box `min..=max` (inclusive, any level):
    /// lookups inside it answer like `new`, outside read no danger. The
    /// short-hop search builds only its window.
    pub fn clipped(zones: &[DangerZone], min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> DangerMap {
        Self::build(zones, Some((min_x, min_z, max_x, max_z)))
    }

    fn build(zones: &[DangerZone], clip: Option<(i32, i32, i32, i32)>) -> DangerMap {
        let mut map = DangerMap::default();
        let mut levels: Vec<i32> = zones.iter().map(|zone| zone.level).collect();
        levels.sort_unstable();
        levels.dedup();
        for level in levels {
            let squares: Vec<(i32, i32, i32, i32)> =
                zones.iter().filter(|zone| zone.level == level).filter_map(|zone| bounds(zone, clip)).collect();
            if squares.is_empty() {
                continue;
            }
            let min_x = squares.iter().map(|square| square.0).min().unwrap_or(0);
            let min_z = squares.iter().map(|square| square.1).min().unwrap_or(0);
            let max_x = squares.iter().map(|square| square.2).max().unwrap_or(0);
            let max_z = squares.iter().map(|square| square.3).max().unwrap_or(0);
            let (width, height) = (max_x - min_x + 1, max_z - min_z + 1);
            if i64::from(width) * i64::from(height) > MAX_BITS {
                let spread = map.spread.get_or_insert_with(HashSet::new);
                for (square_min_x, square_min_z, square_max_x, square_max_z) in squares {
                    for tile_x in square_min_x..=square_max_x {
                        for tile_z in square_min_z..=square_max_z {
                            spread.insert((tile_x, tile_z, level));
                        }
                    }
                }
                continue;
            }
            let row_words = (width as usize).div_ceil(64);
            let mut layer = Layer { level, min_x, min_z, width, height, row_words, bits: vec![0; row_words * height as usize] };
            for (square_min_x, square_min_z, square_max_x, square_max_z) in squares {
                for tile_z in square_min_z..=square_max_z {
                    layer.fill(tile_z - min_z, square_min_x - min_x, square_max_x - min_x);
                }
            }
            map.layers.push(layer);
        }
        map
    }

    /// True when `tile` lies in a square.
    pub fn contains(&self, tile: Tile) -> bool {
        if self.layers.iter().any(|layer| layer.level == tile.level && layer.contains(tile.x, tile.z)) {
            return true;
        }
        self.spread.as_ref().is_some_and(|spread| spread.contains(&(tile.x, tile.z, tile.level)))
    }

    /// Extra cost of stepping onto `tile`.
    pub fn extra(&self, tile: Tile) -> u32 {
        if self.contains(tile) {
            DANGER_STEP_COST
        } else {
            0
        }
    }

    /// No square at all (most searches of a safe bot).
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.spread.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{DangerMap, DangerZone, DANGER_STEP_COST, RADIUS_CLAMP};
    use crate::plan::Tile;
    use std::collections::HashSet;

    /// The old construction, kept as the oracle.
    fn tile_set(zones: &[DangerZone]) -> HashSet<(i32, i32, i32)> {
        let mut tiles = HashSet::new();
        for zone in zones {
            let radius = zone.radius.clamp(0, RADIUS_CLAMP);
            for tile_x in zone.x - radius..=zone.x + radius {
                for tile_z in zone.z - radius..=zone.z + radius {
                    tiles.insert((tile_x, tile_z, zone.level));
                }
            }
        }
        tiles
    }

    fn zones(seed: u64, count: usize) -> Vec<DangerZone> {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as i32
        };
        (0..count)
            .map(|_| DangerZone { x: 3100 + next() % 300, z: 3200 + next() % 300, level: next() % 2, radius: next() % 40 - 3 })
            .collect()
    }

    /// Every tile of the box (and a margin) reads the same as the old
    /// tile set: radius clamp, negative radius, overlaps, two levels,
    /// word boundaries.
    #[test]
    fn bitmap_matches_tile_set() {
        for seed in 0..6 {
            let zones = zones(seed, 40 + seed as usize * 30);
            let oracle = tile_set(&zones);
            let map = DangerMap::new(&zones);
            for level in 0..3 {
                for tile_x in 3050..3450 {
                    for tile_z in 3150..3550 {
                        let tile = Tile { x: tile_x, z: tile_z, level };
                        let want = if oracle.contains(&(tile_x, tile_z, level)) { DANGER_STEP_COST } else { 0 };
                        assert_eq!(map.extra(tile), want, "seed {seed} tile {tile:?}");
                    }
                }
            }
        }
    }

    /// The clipped map answers like the full one inside its window.
    #[test]
    fn clipped_matches_inside_window() {
        let zones = zones(9, 120);
        let full = DangerMap::new(&zones);
        let clipped = DangerMap::clipped(&zones, 3180, 3260, 3230, 3300);
        for tile_x in 3180..=3230 {
            for tile_z in 3260..=3300 {
                for level in 0..2 {
                    let tile = Tile { x: tile_x, z: tile_z, level };
                    assert_eq!(clipped.extra(tile), full.extra(tile), "{tile:?}");
                }
            }
        }
    }

    /// Squares spread past the bitmap bound keep the exact tile set.
    #[test]
    fn spread_squares_stay_exact() {
        let zones = vec![
            DangerZone { x: 0, z: 0, level: 0, radius: 2 },
            DangerZone { x: 9000, z: 9000, level: 0, radius: 1 },
        ];
        let map = DangerMap::new(&zones);
        assert_eq!(map.extra(Tile { x: 2, z: -2, level: 0 }), DANGER_STEP_COST);
        assert_eq!(map.extra(Tile { x: 3, z: 0, level: 0 }), 0);
        assert_eq!(map.extra(Tile { x: 9001, z: 8999, level: 0 }), DANGER_STEP_COST);
        assert_eq!(map.extra(Tile { x: 9001, z: 8999, level: 1 }), 0);
        assert!(DangerMap::new(&[]).is_empty());
    }
}
