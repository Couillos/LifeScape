//! Click stands of a loc: the tiles the server runs a loc op from.
//!
//! OPLOC carries no path: the engine runs the op only when `reachedLoc`
//! (rsmod reach strategy, `Player.inOperableDistance`) holds on the
//! player's current tile; otherwise, with no waypoint and no step taken,
//! `Player.processInteraction` answers "I can't reach that!". The stand
//! tile is therefore the whole decision. For a centrepiece (shape
//! 10/11/22: bank booths) the rule is a side tile of the rotated
//! footprint with no wall on the shared edge — never a diagonal. The
//! rule itself is `lifescape_nav::reach::reached`, the engine port the
//! planner already uses; this module only enumerates and filters.

use std::collections::HashMap;

use lifescape_nav::collision::tile_walkable;
use lifescape_nav::reach;

use crate::client::Bot;
use crate::error::{Error, Result};

/// Engine placement of one loc (`new Loc(level, x, z, type.width,
/// type.length, ..., shape, angle)` in GameMap.ts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocPlacement {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    /// LocType width: `reachedLoc` destWidth.
    pub width: i32,
    /// LocType length: `reachedLoc` destHeight.
    pub length: i32,
    pub shape: u8,
    pub angle: u8,
    /// LocType forceapproach (`reachedLoc` blockAccessFlags).
    pub block_access: u32,
}

/// Every tile the server accepts an op on this loc from, ascending
/// (x, z). Candidates are the rotated footprint and its ring (every
/// reach strategy answers on or beside the loc); each must be standable
/// (`tile_walkable` on its static flags) and pass the server reach rule
/// read on the same flags. Pure: `get_flag` is the only world access.
pub fn stand_tiles(get_flag: &dyn Fn(i32, i32, i32) -> u32, placement: LocPlacement) -> Vec<(i32, i32)> {
    let (span_x, span_z) = rotated_span(placement);
    let mut out = Vec::new();
    for stand_x in placement.x - 1..=placement.x + span_x {
        for stand_z in placement.z - 1..=placement.z + span_z {
            if !tile_walkable(get_flag(stand_x, stand_z, placement.level)) {
                continue;
            }
            if reach::reached(
                get_flag,
                placement.level,
                stand_x,
                stand_z,
                placement.x,
                placement.z,
                placement.width,
                placement.length,
                i32::from(placement.angle),
                i32::from(placement.shape),
                placement.block_access,
            ) {
                out.push((stand_x, stand_z));
            }
        }
    }
    out
}

/// Footprint extent on the map: odd angles swap width and length
/// (`reach::rotate`, same as the engine).
fn rotated_span(placement: LocPlacement) -> (i32, i32) {
    let angle = i32::from(placement.angle);
    (reach::rotate(angle, placement.width, placement.length), reach::rotate(angle, placement.length, placement.width))
}

/// Stands nearest the bot first (Manhattan), ties by tile so the same
/// position always tries the same order.
pub fn nearest_first(stands: &[(i32, i32)], from: (i32, i32)) -> Vec<(i32, i32)> {
    let mut ordered = stands.to_vec();
    ordered.sort_by_key(|stand| ((stand.0 - from.0).abs() + (stand.1 - from.1).abs(), stand.0, stand.1));
    ordered
}

impl Bot {
    /// Stand tiles of the bank booth `loc_id` at (x, z, level) under the
    /// server reach rule, from the plan service's static reads (mapless
    /// workers: never a local read). Shape and angle come from the live
    /// loc when streamed, else the static placement; a tile holding
    /// another loc (or none) fails typed instead of guessing.
    /// forceapproach is not in the gamedata dumps: booths declare none
    /// (`interface_bank/configs/bank_booth.loc`), so it reads 0 — this
    /// helper is scoped to booths for that reason.
    pub fn bank_booth_stands(&mut self, x: i32, z: i32, level: i32, loc_id: u16) -> Result<Vec<(i32, i32)>> {
        let static_read = self.tiles_flags(&[(x, z, level)])?;
        let live = self.world.loc_at(x, z, level);
        let placed = match live {
            Some(entry) => Some(entry),
            None if self.world.loc_removed(x, z, level) => None,
            None => static_read.first().and_then(|read| read.loc),
        };
        let Some((placed_id, shape, angle)) = placed.filter(|(placed_id, _, _)| *placed_id == loc_id) else {
            return Err(Error::Unreachable(format!(
                "booth {loc_id} not at ({x},{z},{level}): live {live:?}, static {:?}",
                static_read.first().and_then(|read| read.loc)
            )));
        };
        let def = self.door_defs(&[placed_id])?.into_iter().next();
        let Some(def) = def.filter(|def| def.width > 0 && def.length > 0) else {
            return Err(Error::State(format!("booth {loc_id}: no footprint in the loc defs")));
        };
        let placement =
            LocPlacement { x, z, level, width: def.width, length: def.length, shape, angle, block_access: 0 };
        let (span_x, span_z) = rotated_span(placement);
        let mut tiles = Vec::new();
        for ring_x in x - 1..=x + span_x {
            for ring_z in z - 1..=z + span_z {
                tiles.push((ring_x, ring_z, level));
            }
        }
        let reads = self.tiles_flags(&tiles)?;
        let flags: HashMap<(i32, i32, i32), u32> =
            reads.iter().map(|read| ((read.tile.x, read.tile.z, read.tile.level), read.flags)).collect();
        // Every tile `stand_tiles` reads is in the batch: the candidate
        // ring is exactly `tiles`. A missing read is a short reply.
        if flags.len() != tiles.len() {
            return Err(Error::Travel(format!("nav-down: tiles_flags answered {} of {} tiles", flags.len(), tiles.len())));
        }
        let get_flag = |flag_x: i32, flag_z: i32, flag_level: i32| {
            flags.get(&(flag_x, flag_z, flag_level)).copied().unwrap_or(lifescape_nav::collision::NULL)
        };
        Ok(stand_tiles(&get_flag, placement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::travel::test_nav_context;

    fn booth(x: i32, z: i32, angle: u8) -> LocPlacement {
        LocPlacement { x, z, level: 0, width: 1, length: 1, shape: 10, angle, block_access: 0 }
    }

    /// Real collision (maps-server.zip): each booth's stands are the side
    /// tiles with no wall on the shared edge. Diagonals never appear (live:
    /// 2303 refused clicks at Varrock east came from the diagonal
    /// (3251,3420) — "I can't reach that!"), nor the counter laterals
    /// (neighbour booths, blocked), nor a wall-separated side (Varrock west
    /// south (3186,3435) has WALL_NORTH: bb4v5d9m lost a market handover
    /// clicking from there).
    #[test]
    fn booth_stands_follow_the_server_reach_rule() {
        let (world, _) = test_nav_context();
        let flags = |x: i32, z: i32, level: i32| world.get(x, z, level);
        let cases: [(LocPlacement, Vec<(i32, i32)>); 6] = [
            (booth(3252, 3419, 0), vec![(3252, 3418), (3252, 3420)]),
            (booth(3256, 3419, 0), vec![(3256, 3418), (3256, 3420), (3257, 3419)]),
            (booth(3186, 3436, 3), vec![(3185, 3436), (3186, 3437), (3187, 3436)]),
            (booth(3091, 3242, 1), vec![(3090, 3242), (3092, 3242)]),
            (booth(3015, 3354, 0), vec![(3015, 3353), (3015, 3355), (3016, 3354)]),
            (booth(2945, 3367, 2), vec![(2945, 3366), (2945, 3368)]),
        ];
        for (placement, expected) in cases {
            let stands = stand_tiles(&flags, placement);
            assert_eq!(stands, expected, "booth ({},{})", placement.x, placement.z);
            for (stand_x, stand_z) in &stands {
                let side = (stand_x - placement.x).abs() + (stand_z - placement.z).abs();
                assert_eq!(side, 1, "side tile only, never a diagonal: ({stand_x},{stand_z})");
            }
        }
        assert!(!stand_tiles(&flags, booth(3252, 3419, 0)).contains(&(3251, 3420)), "live diagonal excluded");
        assert!(!stand_tiles(&flags, booth(3186, 3436, 3)).contains(&(3186, 3435)), "walled south excluded");
    }

    /// Nearest stand first from the bot: at Varrock east the bot waits on
    /// the diagonal (3251,3420), one step from the north stand; the
    /// teller-side south stand (disconnected) comes last.
    #[test]
    fn stands_order_nearest_first() {
        let stands = vec![(3252, 3418), (3252, 3420)];
        assert_eq!(nearest_first(&stands, (3251, 3420)), vec![(3252, 3420), (3252, 3418)]);
        let tie = vec![(3185, 3436), (3187, 3436), (3186, 3437)];
        assert_eq!(nearest_first(&tie, (3186, 3436)), vec![(3185, 3436), (3186, 3437), (3187, 3436)], "ties break by tile");
    }

    /// Footprint ring follows rotation: a 2x1 loc at an odd angle spans
    /// 1 east and 2 north, so its side stands sit on that rotated box.
    #[test]
    fn stand_ring_rotates_with_the_loc() {
        let open = |_: i32, _: i32, _: i32| lifescape_nav::collision::OPEN;
        let placement = LocPlacement { x: 10, z: 10, level: 0, width: 2, length: 1, shape: 10, angle: 1, block_access: 0 };
        assert_eq!(
            stand_tiles(&open, placement),
            vec![(9, 10), (9, 11), (10, 9), (10, 10), (10, 11), (10, 12), (11, 10), (11, 11)],
        );
    }
}
