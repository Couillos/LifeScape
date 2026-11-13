//! Open door leaves lying across a planned walk step.
//!
//! The planner never walls a swung door leaf: it marks the edge
//! (`DOOR_LEAF_*`, `nav::collision::gate_swing`) and pays
//! `DOOR_LEAF_STEP_COST` to cross it, because a shut door leaves that
//! edge free (walled, the leaf split 22 F2P rooms in two). When no
//! detour exists the plan crosses the marked edge, and if the door
//! stands open at that moment the leaf is a real wall: the server
//! refuses the step. Live 2026-11-13 t01-21040: Falador kitchen
//! (2988,3367) is only enterable across the leaf edge of door 1512
//! ((2989,3368) angle south); open, its leaf 1515 stands on (2989,3367)
//! angle west and the bot stalled there, "I can't reach that!".
//!
//! The walk executor closes such a leaf before the step: the doors
//! content (`doors.rs2` `[oploc1,_door_opened]`, `doubledoors.rs2`
//! `_door_left_opened` / `_door_right_opened`) closes every marked leaf
//! kind on op1, and the step's source tile always reaches the wall (on
//! it, or across the very edge). The static leaf mark is required, so a
//! live wall that is not a planner-marked door leaf is never clicked.

#![forbid(unsafe_code)]

use lifescape_nav::collision::leaf_step_extra;

use crate::client::Bot;
use crate::error::{Error, Result};
use crate::world::LocValue;

/// Op closing an open door leaf (`[oploc1,_door_opened]` and the double
/// door `_opened` handlers).
const LEAF_CLOSE_OP: u8 = 1;
/// Leaf close watch: same swing as an opening (`DOOR_SWING_WAIT`
/// measured 17 ticks send-to-visible), same bound.
const LEAF_CLOSE_WAIT: u64 = 23;
/// Straight wall shape (`wall_straight`): the only shape a swung leaf takes.
const WALL_STRAIGHT: u8 = 0;

/// One open leaf blocking a step: its live id and tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BlockingLeaf {
    pub loc: u16,
    pub x: i32,
    pub z: i32,
}

/// Unit offset of the edge a straight wall covers on its own tile:
/// angle 0 west, 1 north, 2 east, 3 south (engine `LocAngle`).
fn wall_side(angle: u8) -> (i32, i32) {
    match angle & 3 {
        0 => (-1, 0),
        1 => (0, 1),
        2 => (1, 0),
        _ => (0, -1),
    }
}

/// Live straight wall that blocks the step `from` -> `to` and that the
/// bot on `from` can reach: a wall on `from` covering the side the step
/// leaves by (orthogonal or either side of a diagonal), or a wall on
/// `to` covering the edge an orthogonal step enters by. A wall on `to`
/// brushed by a diagonal is out of reach from `from`: left alone.
pub(crate) fn live_wall_across(
    from: (i32, i32),
    to: (i32, i32),
    loc_at: impl Fn(i32, i32) -> Option<LocValue>,
) -> Option<BlockingLeaf> {
    let (step_x, step_z) = (to.0 - from.0, to.1 - from.1);
    if step_x.abs() > 1 || step_z.abs() > 1 || (step_x, step_z) == (0, 0) {
        return None;
    }
    if let Some((loc, shape, angle)) = loc_at(from.0, from.1) {
        let side = wall_side(angle);
        if shape == WALL_STRAIGHT && ((step_x != 0 && side == (step_x, 0)) || (step_z != 0 && side == (0, step_z))) {
            return Some(BlockingLeaf { loc, x: from.0, z: from.1 });
        }
    }
    if step_x == 0 || step_z == 0 {
        if let Some((loc, shape, angle)) = loc_at(to.0, to.1) {
            if shape == WALL_STRAIGHT && wall_side(angle) == (-step_x, -step_z) {
                return Some(BlockingLeaf { loc, x: to.0, z: to.1 });
            }
        }
    }
    None
}

/// First step of `steps` (walked from `start`) with a live wall across
/// it: (index of the step's target in `steps`, the wall, source, target).
fn first_wall(
    start: (i32, i32),
    steps: &[(i32, i32)],
    loc_at: impl Fn(i32, i32) -> Option<LocValue>,
) -> Option<(usize, BlockingLeaf, (i32, i32), (i32, i32))> {
    let mut prev = start;
    for (index, tile) in steps.iter().enumerate() {
        if let Some(wall) = live_wall_across(prev, *tile, &loc_at) {
            return Some((index, wall, prev, *tile));
        }
        prev = *tile;
    }
    None
}

impl Bot {
    /// First step of `steps` crossing a live open door leaf the planner
    /// marked: (index of the step's target, the leaf). A live wall on an
    /// unmarked edge is not a leaf and returns None (the walk then meets
    /// it as usual). The mark check costs one `tiles_flags` batch, and
    /// only when a live wall lies across a step.
    pub(crate) fn leaf_ahead(&mut self, steps: &[(i32, i32)]) -> Result<Option<(usize, BlockingLeaf)>> {
        let level = self.world.level;
        let start = (self.world.x, self.world.z);
        let Some((index, wall, from, to)) = first_wall(start, steps, |x, z| self.world.loc_at(x, z, level)) else {
            return Ok(None);
        };
        let rows = self.tiles_flags(&[(from.0, from.1, level), (to.0, to.1, level)])?;
        let (Some(here), Some(next)) = (rows.first(), rows.get(1)) else {
            return Err(Error::Travel("nav-down: short tiles_flags reply for a leaf check".to_string()));
        };
        if leaf_step_extra(here.flags, next.flags, to.0 - from.0, to.1 - from.1) == 0 {
            return Ok(None);
        }
        Ok(Some((index, wall)))
    }

    /// Close an open leaf from the step's source tile and wait until the
    /// live loc leaves its tile. A leaf still standing after the watch
    /// is a typed `StepRefused` (never a blind step into it).
    pub(crate) fn close_leaf(&mut self, leaf: BlockingLeaf, deadline: u64) -> Result<()> {
        let level = self.world.level;
        self.op_loc(LEAF_CLOSE_OP, leaf.x, leaf.z, leaf.loc)?;
        let watched = self.world.ticks + LEAF_CLOSE_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            if self.world.loc_at(leaf.x, leaf.z, level).map(|loc| loc.0) != Some(leaf.loc) {
                return Ok(());
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("closing leaf {} timed out at {:?}", leaf.loc, self.position())));
            }
        }
        Err(Error::StepRefused(format!(
            "open leaf {} at ({},{},{}) never closed from {:?}; last game: {}",
            leaf.loc,
            leaf.x,
            leaf.z,
            level,
            self.position(),
            super::travel::last_message(self),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn locs(entries: &[((i32, i32), LocValue)]) -> impl Fn(i32, i32) -> Option<LocValue> {
        let map: HashMap<(i32, i32), LocValue> = entries.iter().copied().collect();
        move |x, z| map.get(&(x, z)).copied()
    }

    /// t01-21040: leaf 1515 on (2989,3367) angle 0 (west) blocks the step
    /// west into the Falador kitchen (2988,3367); the bot stands on it.
    #[test]
    fn falador_kitchen_leaf_blocks_the_step_west() {
        let live = locs(&[((2989, 3367), (1515, 0, 0))]);
        assert_eq!(
            live_wall_across((2989, 3367), (2988, 3367), &live),
            Some(BlockingLeaf { loc: 1515, x: 2989, z: 3367 })
        );
        // Same edge walked the other way: the wall sits on the target.
        assert_eq!(
            live_wall_across((2988, 3367), (2989, 3367), &live),
            Some(BlockingLeaf { loc: 1515, x: 2989, z: 3367 })
        );
        // Steps that never touch the west edge pass.
        assert_eq!(live_wall_across((2989, 3367), (2990, 3367), &live), None);
        assert_eq!(live_wall_across((2989, 3367), (2989, 3366), &live), None);
        assert_eq!(live_wall_across((2990, 3367), (2989, 3367), &live), None);
    }

    /// A diagonal leaving the leaf tile by the walled side is blocked
    /// (reachable: the bot stands on the wall); a diagonal brushing a
    /// wall on its target is out of click reach and left to the server.
    #[test]
    fn diagonals_close_only_from_the_leaf_tile() {
        let live = locs(&[((2989, 3367), (1515, 0, 0))]);
        assert!(live_wall_across((2989, 3367), (2988, 3366), &live).is_some());
        assert_eq!(live_wall_across((2988, 3366), (2989, 3367), &live), None);
    }

    /// Ground decor and non-straight shapes never count; the first
    /// blocked step of a leg is reported with its index.
    #[test]
    fn first_wall_names_the_step_index() {
        let live = locs(&[((2990, 3367), (916, 22, 0)), ((2989, 3367), (1515, 0, 0))]);
        let steps = [(2990, 3367), (2989, 3367), (2988, 3367)];
        let found = first_wall((2991, 3367), &steps, &live).expect("leaf on the leg");
        assert_eq!(found.0, 2, "the step into (2988,3367) is the third");
        assert_eq!((found.2, found.3), ((2989, 3367), (2988, 3367)));
        assert_eq!(first_wall((2991, 3367), &steps[..2], &live), None);
    }

    /// Through the real service snapshot: t01's refused step finds the
    /// live leaf (marked edge), the same live wall on the unmarked north
    /// edge of the tile is not a leaf, and no live wall costs no batch.
    #[test]
    fn leaf_ahead_reads_the_planner_mark_from_the_service() {
        use crate::client::travel::test_nav_context;
        use lifescape_nav::service::{test_socket, PlanClient, Service, Snapshot};
        let (world, tables) = test_nav_context();
        let service = Service::start(Snapshot::new(world, tables, "test".to_string()), &test_socket("sdk-leaf"), 1)
            .expect("service starts");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 2989;
        bot.world.z = 3367;
        bot.world.level = 0;
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test".to_string());
        let kitchen = [(2988, 3367)];
        assert_eq!(bot.leaf_ahead(&kitchen).expect("no live wall"), None);
        bot.world.locs.insert((2989, 3367, 0), Some((1515, 0, 0)));
        assert_eq!(
            bot.leaf_ahead(&kitchen).expect("service answers"),
            Some((0, BlockingLeaf { loc: 1515, x: 2989, z: 3367 })),
        );
        bot.world.locs.insert((2989, 3367, 0), Some((1515, 0, 1)));
        assert_eq!(bot.leaf_ahead(&[(2989, 3368)]).expect("service answers"), None, "north edge carries no leaf mark");
    }

    /// The static mark the planner paints is what proves a door leaf:
    /// leaf (2989,3367) angle west marks its west edge and the east edge
    /// of (2988,3367), exactly the step t01 was refused.
    #[test]
    fn the_planner_mark_matches_the_live_leaf_edge() {
        use lifescape_nav::collision::{open_door_leaf, DOOR_LEAF_EAST, DOOR_LEAF_WEST};
        assert_eq!(open_door_leaf(2989, 3368, 3), (2989, 3367, 0));
        assert!(leaf_step_extra(DOOR_LEAF_WEST, DOOR_LEAF_EAST, -1, 0) > 0);
        assert_eq!(leaf_step_extra(DOOR_LEAF_WEST, 0, 1, 0), 0);
    }
}
