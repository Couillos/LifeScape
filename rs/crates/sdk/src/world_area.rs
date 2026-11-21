//! What the live world forgets when tiles leave the client's view: the
//! 8x8 zone reset of a full follows, and everything outside the build
//! area on a region rebuild (both as `Client.ts` does).

use super::{Tile, World};

/// Tiles per side of the build area (`BuildArea.SIZE` in Client.ts).
const BUILD_AREA_SIZE: i32 = 104;

impl World {
    /// Full follows: the server "completely resets the client zone to
    /// default — no objs and the original locs" (Zone.ts writeFullFollows,
    /// Client.ts clears groundObj and reverts locChanges), then re-sends
    /// every obj and changed loc still there. A zone re-enters view this
    /// way, so anything taken or despawned while it was out of view would
    /// otherwise stay a ghost forever. Static spawns are shadowed until
    /// their OBJ_ADD comes back.
    pub(super) fn reset_zone(&mut self) {
        let (min_x, min_z, level) = (self.zone_x << 3, self.zone_z << 3, self.level);
        let inside = |tile: &Tile| tile.2 == level && (min_x..min_x + 8).contains(&tile.0) && (min_z..min_z + 8).contains(&tile.1);
        self.objs.retain(|(tile, _), _| !inside(tile));
        let shadowed: Vec<(Tile, u16)> = self.static_objs.keys().filter(|(tile, _)| inside(tile)).copied().collect();
        for key in shadowed {
            self.objs.insert(key, None);
        }
        self.locs.retain(|tile, _| !inside(tile));
    }

    /// Region rebuild: Client.ts unlinks every loc change and nulls every
    /// ground obj outside the new 104x104 build area. Kept, a rock seen
    /// mined out an hour ago stayed « mined out » seen from the bank, and
    /// miners waited there for a respawn that had long happened (live
    /// 2026-11-21: fiona_park waited at Edgeville on Al Kharid coal,
    /// amber_moore at the Varrock west bank on the south-west mine's
    /// adamantite). Out of view, a tile reads its static default again;
    /// the full follows of its zone brings the live state back on entry.
    pub(super) fn forget_outside_build_area(&mut self) {
        let (min_x, min_z) = (self.base_x, self.base_z);
        let inside = |tile: &Tile| (min_x..min_x + BUILD_AREA_SIZE).contains(&tile.0) && (min_z..min_z + BUILD_AREA_SIZE).contains(&tile.1);
        self.locs.retain(|tile, _| inside(tile));
        self.objs.retain(|(tile, _), _| inside(tile));
    }
}

#[cfg(test)]
mod tests {
    use super::super::World;

    /// A rock mined out at (360,370) is a loc change; the bot walks off
    /// and the region rebuilds around (1000,1000): the change is gone, the
    /// tile reads its static rock again. A change inside the new area stays.
    #[test]
    fn rebuild_forgets_loc_changes_and_objs_outside_the_build_area() {
        let mut world = World::new();
        world.feed("REBUILD_NORMAL", &[0, 50, 0, 50]).unwrap();
        world.static_locs.insert((360, 370, 0), (2090, 10, 0));
        world.locs.insert((360, 370, 0), Some((450, 10, 0)));
        world.locs.insert((361, 371, 0), None);
        world.objs.insert(((362, 372, 0), 1739), Some(1));
        world.static_objs.insert(((363, 373, 0), 526), 1);
        world.objs.insert(((363, 373, 0), 526), None);
        // New zone (125,125) -> base (119 << 3) = 952: area 952..1056.
        world.locs.insert((1000, 1000, 1), None);
        world.objs.insert(((1001, 1001, 0), 1739), Some(2));
        world.feed("REBUILD_NORMAL", &[0, 125, 0, 125]).unwrap();
        assert_eq!(world.loc_at(360, 370, 0), Some((2090, 10, 0)), "static rock again");
        assert!(!world.loc_removed(361, 371, 0), "cleared tile forgotten");
        assert!(world.ground_items(362, 372, 0, 0, None).is_empty(), "far live obj forgotten");
        assert_eq!(world.ground_items(363, 373, 0, 0, Some(526)).len(), 1, "far static spawn unshadowed");
        assert!(world.loc_removed(1000, 1000, 1), "change inside the new area kept, every level");
        assert_eq!(world.ground_items(1001, 1001, 0, 0, Some(1739)).len(), 1, "obj inside kept");
    }

    /// The edges: the area is [base, base + 104) on both axes.
    #[test]
    fn rebuild_keeps_the_last_tile_of_the_area_and_drops_the_next() {
        let mut world = World::new();
        world.feed("REBUILD_NORMAL", &[0, 50, 0, 50]).unwrap();
        let base = 44 << 3;
        world.locs.insert((base, base + 103, 0), None);
        world.locs.insert((base + 104, base, 0), None);
        world.locs.insert((base - 1, base, 0), None);
        world.feed("REBUILD_NORMAL", &[0, 50, 0, 50]).unwrap();
        assert!(world.loc_removed(base, base + 103, 0));
        assert!(!world.loc_removed(base + 104, base, 0));
        assert!(!world.loc_removed(base - 1, base, 0));
    }
}
