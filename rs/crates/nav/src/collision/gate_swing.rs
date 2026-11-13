//! Swung-open double gate leaves, painted as static walls.
//!
//! Content `general_use/scripts/gates.rs2` (`open_gate`): opening a
//! closed double gate deletes both leaves and re-adds them for 500 ticks
//! as straight walls rotated `angle + 3`, the main leaf one tile past the
//! fence and the outer leaf two tiles past it (`gate_set_close(angle,
//! 1|2)`, both offsets taken from the main leaf). Those walls cut the
//! tiles just past the gate: a plan that walked across them was refused
//! live (Falador and Lumbridge field gates, 2026-11-12: ~5000 refusals,
//! the same-path replan refused again, then the scripts walked back
//! through the gate). With a dozen bots crossing, these gates stand
//! open most of the time, so the planner reads the open leaves as
//! walls for good: a shut gate only costs a tile or two of detour,
//! an open one no longer refuses the step.

use super::flags::{ANGLE_EAST, ANGLE_NORTH, ANGLE_SOUTH, ANGLE_WEST, WALL_STRAIGHT};
use super::loc_collision::CollisionMap;

/// Role of a closed double gate leaf (content categories
/// `gate_main_closed` / `gate_outer_closed`, `content/pack/category.pack`
/// ids 28 / 29): the click on either opens both, the main leaf decides
/// where they swing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateLeaf {
    Main,
    Outer,
}

/// Category id of `gate_main_closed` in the engine pack.
pub const GATE_MAIN_CLOSED_CATEGORY: i64 = 28;
/// Category id of `gate_outer_closed` in the engine pack.
pub const GATE_OUTER_CLOSED_CATEGORY: i64 = 29;

/// Gate role from a loctype category id, `None` for every other loc.
pub fn gate_leaf_of_category(category: i64) -> Option<GateLeaf> {
    match category {
        GATE_MAIN_CLOSED_CATEGORY => Some(GateLeaf::Main),
        GATE_OUTER_CLOSED_CATEGORY => Some(GateLeaf::Outer),
        _ => None,
    }
}

/// Server `gate_set_close(angle, 1)`: one tile towards the swing side.
pub fn swing_offset(angle: u8) -> (i32, i32) {
    match angle {
        ANGLE_WEST => (-1, 0),
        ANGLE_NORTH => (0, 1),
        ANGLE_EAST => (1, 0),
        ANGLE_SOUTH => (0, -1),
        _ => (0, 0),
    }
}

/// Where the open leaves land for one closed main leaf: (x, z, angle)
/// of the main leaf, then of the outer leaf when the gate has one.
pub fn open_leaves(main_x: i32, main_z: i32, angle: u8, has_outer: bool) -> Vec<(i32, i32, u8)> {
    let (step_x, step_z) = swing_offset(angle);
    let open_angle = (angle + 3) % 4;
    let mut leaves = vec![(main_x + step_x, main_z + step_z, open_angle)];
    if has_outer {
        leaves.push((main_x + step_x * 2, main_z + step_z * 2, open_angle));
    }
    leaves
}

/// One closed gate leaf as the square loader met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosedLeaf {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub leaf: GateLeaf,
    pub shape: u8,
    pub angle: u8,
}

/// Paint the open leaves of every closed main leaf of one 64x64 square
/// (`leaves`: the gate leaves that square loaded). The outer leaf is the
/// server's: a closed outer leaf within one tile of the main one (a pair
/// split across two squares keeps its main leaf only). Walk walls only,
/// never projectile bits.
pub fn paint_gate_swings(collision: &mut CollisionMap, leaves: &[ClosedLeaf]) {
    for main in leaves.iter().filter(|leaf| leaf.leaf == GateLeaf::Main && leaf.shape == WALL_STRAIGHT) {
        let has_outer = leaves.iter().any(|other| {
            other.leaf == GateLeaf::Outer
                && other.level == main.level
                && (other.x - main.x).abs() <= 1
                && (other.z - main.z).abs() <= 1
        });
        for (leaf_x, leaf_z, leaf_angle) in open_leaves(main.x, main.z, main.angle, has_outer) {
            collision.change_wall(leaf_x, leaf_z, main.level, leaf_angle, WALL_STRAIGHT, false, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{step_open, WALL_EAST, WALL_NORTH, WALL_SOUTH, WALL_WEST};

    fn closed(x: i32, z: i32, leaf: GateLeaf, angle: u8) -> ClosedLeaf {
        ClosedLeaf { x, z, level: 0, leaf, shape: WALL_STRAIGHT, angle }
    }

    /// Live: 1551 (3031,3313) angle north -> 1552 (3031,3314) and 1556
    /// (3031,3315), both angle west. Lumbridge 1551 (3236,3296) angle
    /// east -> (3237,3296) and (3238,3296), both angle north.
    #[test]
    fn open_leaves_match_live_positions() {
        assert_eq!(open_leaves(3031, 3313, ANGLE_NORTH, true), vec![(3031, 3314, ANGLE_WEST), (3031, 3315, ANGLE_WEST)]);
        assert_eq!(open_leaves(3236, 3296, ANGLE_EAST, true), vec![(3237, 3296, ANGLE_NORTH), (3238, 3296, ANGLE_NORTH)]);
        assert_eq!(open_leaves(3253, 3266, ANGLE_WEST, true), vec![(3252, 3266, ANGLE_SOUTH), (3251, 3266, ANGLE_SOUTH)]);
        assert_eq!(open_leaves(2924, 3292, ANGLE_SOUTH, false), vec![(2924, 3291, ANGLE_EAST)]);
    }

    #[test]
    fn categories_name_the_two_closed_leaves() {
        assert_eq!(gate_leaf_of_category(28), Some(GateLeaf::Main));
        assert_eq!(gate_leaf_of_category(29), Some(GateLeaf::Outer));
        assert_eq!(gate_leaf_of_category(41), None);
        assert_eq!(gate_leaf_of_category(-1), None);
    }

    /// Falador cow field pair, painted from the main leaf: both open
    /// leaves wall their west edge, the gate crossing itself stays free.
    #[test]
    fn painted_pair_walls_the_swing_side_only() {
        let leaves = [closed(3031, 3313, GateLeaf::Main, ANGLE_NORTH), closed(3032, 3313, GateLeaf::Outer, ANGLE_NORTH)];
        let mut map = CollisionMap::new();
        for x in 3028..3036 {
            for z in 3310..3318 {
                map.set(x, z, 0, 0);
            }
        }
        paint_gate_swings(&mut map, &leaves);
        assert_ne!(map.get(3031, 3314, 0) & WALL_WEST, 0);
        assert_ne!(map.get(3031, 3315, 0) & WALL_WEST, 0);
        assert_ne!(map.get(3030, 3314, 0) & WALL_EAST, 0);
        assert!(!step_open(&map, 3031, 3314, 0, -1, 0), "refused live step stays shut");
        assert!(step_open(&map, 3031, 3313, 0, 0, 1), "crossing north stays open");
        assert!(step_open(&map, 3031, 3314, 0, 0, 1), "walking on along the leaf stays open");
        assert_eq!(map.get(3031, 3316, 0) & (WALL_WEST | WALL_SOUTH | WALL_NORTH), 0, "nothing past the outer leaf");
    }

    /// A lone main leaf (no closed outer leaf beside it) swings alone.
    #[test]
    fn lone_main_leaf_paints_one_wall() {
        let leaves = [closed(10, 10, GateLeaf::Main, ANGLE_EAST)];
        let mut map = CollisionMap::new();
        for x in 8..14 {
            for z in 8..14 {
                map.set(x, z, 0, 0);
            }
        }
        paint_gate_swings(&mut map, &leaves);
        assert_ne!(map.get(11, 10, 0) & WALL_NORTH, 0);
        assert_eq!(map.get(12, 10, 0) & WALL_NORTH, 0);
    }
}
