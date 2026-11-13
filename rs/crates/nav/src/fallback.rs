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
}
