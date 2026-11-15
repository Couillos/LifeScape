//! Blocked-destination fallback: which reachable tile stands in for a
//! target the walk cannot end on (booth, counter, tree, water).
//!
//! `Tile::fallback_rank` orders candidates by level, Chebyshev, then
//! `walled_off`, then cost. Only a neighbour with a wall between it and
//! the target is demoted; an open diagonal or side keeps competing on
//! cost, so a bot already on a valid fallback tile stays there (shore
//! beside water: (3087,3228) for (3086,3227), plan empty). Live
//! 2026-11-13: the Falador west booth (2945,3367) adjusted to the
//! cheaper diagonal (2944,3366), outside the bank's west wall; from
//! there the nearest stand was the sealed teller side (2945,3366) and
//! every bank trip burned an exhaustive NoRoute before walking round to
//! the customer stand (2945,3368).

use crate::collision::{
    WALL_EAST, WALL_NORTH, WALL_NORTH_EAST, WALL_NORTH_WEST, WALL_SOUTH, WALL_SOUTH_EAST, WALL_SOUTH_WEST, WALL_WEST,
};
use crate::plan::Tile;

/// Wall flag on the edge one unit step (`step_x`, `step_z`) away.
fn edge_wall(step_x: i32, step_z: i32) -> u32 {
    match (step_x, step_z) {
        (1, 0) => WALL_EAST,
        (-1, 0) => WALL_WEST,
        (0, 1) => WALL_NORTH,
        (0, -1) => WALL_SOUTH,
        _ => 0,
    }
}

/// Diagonal-corner wall flag towards (`step_x`, `step_z`).
fn corner_wall(step_x: i32, step_z: i32) -> u32 {
    match (step_x, step_z) {
        (1, 1) => WALL_NORTH_EAST,
        (-1, 1) => WALL_NORTH_WEST,
        (1, -1) => WALL_SOUTH_EAST,
        (-1, -1) => WALL_SOUTH_WEST,
        _ => 0,
    }
}

/// True when a wall stands between `here` and its same-level neighbour
/// `dest` (Chebyshev 1), read on both tiles' flags (`flags` is the
/// search's view, usable doors open): the shared edge for a side, the
/// four edges meeting at the shared corner (and corner walls) for a
/// diagonal. Loc footprints never count: the target itself is blocked.
/// Farther tiles, other levels and `here == dest` are never walled off.
pub(crate) fn walled_off(flags: &mut dyn FnMut(Tile) -> u32, here: Tile, dest: Tile) -> bool {
    if here.level != dest.level {
        return false;
    }
    let (step_x, step_z) = (dest.x - here.x, dest.z - here.z);
    if step_x.abs() > 1 || step_z.abs() > 1 || (step_x, step_z) == (0, 0) {
        return false;
    }
    let here_mask = edge_wall(step_x, 0) | edge_wall(0, step_z) | corner_wall(step_x, step_z);
    let dest_mask = edge_wall(-step_x, 0) | edge_wall(0, -step_z) | corner_wall(-step_x, -step_z);
    flags(here) & here_mask != 0 || flags(dest) & dest_mask != 0
}

/// First underground row: cellars and dungeons (map squares z >= 100)
/// lie 6400 tiles north of the surface they sit under, on the same
/// level, joined to it only by rides.
pub(crate) const UNDERGROUND_Z: i32 = 6400;

/// Slack past the best fallback Chebyshev before a tile is pruned.
const FALLBACK_SLACK: i32 = 64;

/// `tile` moved onto `onto`'s plane (surface or underground): the
/// underground map lies under the surface it joins, 6400 rows north.
fn on_plane_of(tile: Tile, onto: Tile) -> Tile {
    let shift = match (tile.z >= UNDERGROUND_Z, onto.z >= UNDERGROUND_Z) {
        (true, false) => -UNDERGROUND_Z,
        (false, true) => UNDERGROUND_Z,
        _ => 0,
    };
    Tile { z: tile.z + shift, ..tile }
}

/// Blocked-destination prune (`dijkstra`, `walk_costs`): `next` lies so
/// far past the best fallback (`best_chebyshev`) that walking on from it
/// can never improve the answer. Measured on the destination's plane: a
/// raw Chebyshev across the surface/underground split reads ~6400 and
/// cut the way to the true closest tile when it runs underground. Live
/// 2026-11-14 (alicefield, hill giants (3109,9841) -> Edgeville booth
/// (3095,3489)): the cellar ladder put the search in the brass-key
/// house, 36 tiles off the booth, every dungeon tile then read ~6350
/// away and was cut, the trapdoor exit was never reached and the house
/// became the adjusted target (SDK: Disconnected, adjusted too far).
pub(crate) fn past_fallback(next: Tile, dest: Tile, best_chebyshev: i32) -> bool {
    next.level == dest.level && on_plane_of(next, dest).chebyshev(dest) > best_chebyshev + FALLBACK_SLACK
}

/// Rings scanned for the rank floor (bounded work per blocked search).
const FLOOR_RINGS: i32 = 16;

/// Best fallback rank prefix (level, Chebyshev, walled) ANY tile could
/// reach for a blocked `dest`: the nearest same-level ring holding a
/// walkable in-bounds tile (`probe` gives the search's flags, `None`
/// out of bounds), an open neighbour before a walled one. A search that
/// pops a tile with exactly this prefix can stop there: pops come in
/// cost order, so no later tile ranks better (CPU 2026-11-14). `None`
/// past `FLOOR_RINGS`: the search sweeps as before.
pub(crate) fn rank_floor(probe: &mut dyn FnMut(Tile) -> Option<u32>, dest: Tile) -> Option<(i32, i32, bool)> {
    for ring in 1..=FLOOR_RINGS {
        let mut walled_only = false;
        for step_x in -ring..=ring {
            for step_z in -ring..=ring {
                if step_x.abs() != ring && step_z.abs() != ring {
                    continue;
                }
                let tile = Tile { x: dest.x + step_x, z: dest.z + step_z, level: dest.level };
                let Some(flags) = probe(tile) else { continue };
                if !crate::collision::tile_walkable(flags) {
                    continue;
                }
                if ring > 1 {
                    return Some((0, ring, false));
                }
                if walled_off(&mut |near| probe(near).unwrap_or(0), tile, dest) {
                    walled_only = true;
                } else {
                    return Some((0, 1, false));
                }
            }
        }
        if walled_only {
            return Some((0, 1, true));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::walled_off;
    use crate::collision::{LOC, WALL_EAST, WALL_WEST};
    use crate::plan::Tile;

    /// Falador west booth flags: the outside diagonal and the outside
    /// side sit behind the bank's west wall and lose to the customer
    /// stand however cheap they are.
    #[test]
    fn walled_neighbours_lose_to_the_open_stand() {
        let booth = Tile { x: 2945, z: 3367, level: 0 };
        let customer = Tile { x: 2945, z: 3368, level: 0 };
        let outside_diagonal = Tile { x: 2944, z: 3366, level: 0 };
        let outside_walled = Tile { x: 2944, z: 3367, level: 0 };
        let mut flags = |tile: Tile| match (tile.x, tile.z) {
            (2944, 3366) | (2944, 3367) => WALL_EAST,
            (2945, 3367) => WALL_WEST | LOC,
            _ => 0,
        };
        assert!(!walled_off(&mut flags, customer, booth));
        assert!(walled_off(&mut flags, outside_diagonal, booth), "diagonal across the west wall");
        assert!(walled_off(&mut flags, outside_walled, booth), "wall on the shared edge");
        let customer_rank = customer.fallback_rank(booth, 17, false);
        assert!(customer_rank < outside_diagonal.fallback_rank(booth, 1, true));
        assert!(customer_rank < outside_walled.fallback_rank(booth, 1, true));
    }

    /// Open diagonal (shore beside water): never demoted, so the tile
    /// the bot stands on (cost 0) keeps winning over an open side.
    #[test]
    fn open_diagonal_keeps_the_cheaper_tile() {
        let water = Tile { x: 3086, z: 3227, level: 0 };
        let shore = Tile { x: 3087, z: 3228, level: 0 };
        let side = Tile { x: 3087, z: 3227, level: 0 };
        let mut flags = |_: Tile| 0;
        assert!(!walled_off(&mut flags, shore, water));
        assert!(!walled_off(&mut flags, side, water));
        assert!(shore.fallback_rank(water, 0, false) < side.fallback_rank(water, 1, false));
    }

    /// Edgeville booth (3095,3489) with the best fallback in the key
    /// house (36 away): the dungeon under Edgeville stays in the search
    /// (gate (3145,9871) reads 50 on the surface plane), a far surface
    /// tile and the far underground still go.
    #[test]
    fn underground_reads_on_the_surface_plane() {
        let booth = Tile { x: 3095, z: 3489, level: 0 };
        assert!(!super::past_fallback(Tile { x: 3145, z: 9871, level: 0 }, booth, 36));
        assert!(!super::past_fallback(Tile { x: 3109, z: 9841, level: 0 }, booth, 36));
        assert!(super::past_fallback(Tile { x: 3300, z: 3489, level: 0 }, booth, 36));
        assert!(super::past_fallback(Tile { x: 3300, z: 9889, level: 0 }, booth, 36));
        assert!(!super::past_fallback(Tile { x: 3300, z: 3489, level: 1 }, booth, 36), "other levels never prune");
    }
}
