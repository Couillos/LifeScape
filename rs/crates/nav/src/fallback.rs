//! Blocked-destination fallback: which reachable tile stands in for a
//! target the walk cannot end on (booth, counter, tree, water).
//!
//! `Tile::fallback_rank` orders candidates by level, Chebyshev, then
//! this test, then cost. A side neighbour with no wall on the shared
//! edge is where the engine's centrepiece reach accepts an op; a
//! diagonal, or a side behind a wall, never is. Live 2026-11-13: the
//! Falador west booth (2945,3367) adjusted to the cheaper diagonal
//! (2944,3366), outside the bank's west wall; from there the nearest
//! stand was the sealed teller side (2945,3366) and every bank trip
//! burned an exhaustive NoRoute before walking round to the customer
//! stand (2945,3368).

use crate::plan::Tile;

/// True when `here` touches `dest` by a side with no wall on the shared
/// edge, read on both tiles' flags (`flags` is the search's view, usable
/// doors open). Diagonals and other levels never touch.
pub(crate) fn touches_side(flags: &mut dyn FnMut(Tile) -> u32, here: Tile, dest: Tile) -> bool {
    use crate::collision::{WALL_EAST, WALL_NORTH, WALL_SOUTH, WALL_WEST};
    if here.level != dest.level {
        return false;
    }
    let (here_wall, dest_wall) = match (dest.x - here.x, dest.z - here.z) {
        (1, 0) => (WALL_EAST, WALL_WEST),
        (-1, 0) => (WALL_WEST, WALL_EAST),
        (0, 1) => (WALL_NORTH, WALL_SOUTH),
        (0, -1) => (WALL_SOUTH, WALL_NORTH),
        _ => return false,
    };
    flags(here) & here_wall == 0 && flags(dest) & dest_wall == 0
}

#[cfg(test)]
mod tests {
    use super::touches_side;
    use crate::collision::{LOC, WALL_EAST, WALL_WEST};
    use crate::plan::Tile;

    /// At equal Chebyshev a touching side tile beats a cheaper diagonal
    /// or a wall-separated side (Falador west booth flags).
    #[test]
    fn fallback_rank_prefers_touching_side() {
        let booth = Tile { x: 2945, z: 3367, level: 0 };
        let customer = Tile { x: 2945, z: 3368, level: 0 };
        let outside_diagonal = Tile { x: 2944, z: 3366, level: 0 };
        let outside_walled = Tile { x: 2944, z: 3367, level: 0 };
        let mut flags = |tile: Tile| match (tile.x, tile.z) {
            (2944, 3367) => WALL_EAST,
            (2945, 3367) => WALL_WEST | LOC,
            _ => 0,
        };
        assert!(touches_side(&mut flags, customer, booth));
        assert!(!touches_side(&mut flags, outside_diagonal, booth), "diagonal never touches");
        assert!(!touches_side(&mut flags, outside_walled, booth), "wall on the shared edge");
        let customer_rank = customer.fallback_rank(booth, 17, true);
        assert!(customer_rank < outside_diagonal.fallback_rank(booth, 1, false));
        assert!(customer_rank < outside_walled.fallback_rank(booth, 1, false));
    }
}
