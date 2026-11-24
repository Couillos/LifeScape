#![forbid(unsafe_code)]
//! The one world reader of the navigation searches (lot N1, 2026-11-24).
//!
//! `plan::dijkstra`, `costs::plan_costs_capped`, `fast_hop::short_hop`
//! and the sealed-pocket flood used to call `StaticWorld` through four
//! private copies of the same reads (flags, door, F2P zone, toll). They
//! now go through this type: the seam where the dense grid of lot N2
//! lives, so one change speeds up every search. A reader is made once per
//! search and is cheap to copy around by reference.

use crate::collision::{CollisionMap, LocDef};
use crate::plan::Tile;
use crate::static_world::StaticWorld;

/// Static reads of one search.
pub(crate) struct Reader<'a> {
    world: &'a StaticWorld,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(world: &'a StaticWorld) -> Reader<'a> {
        Reader { world }
    }

    /// Collision flags of one tile (`StaticWorld::get`).
    pub(crate) fn flags(&self, tile: Tile) -> u32 {
        self.world.get(tile.x, tile.z, tile.level)
    }

    /// Closed door recorded on this tile, if any.
    pub(crate) fn door_at(&self, tile: Tile) -> Option<(u16, u8, u8)> {
        self.world.door_at(tile.x, tile.z, tile.level)
    }

    pub(crate) fn is_toll(&self, tile: Tile) -> bool {
        self.world.is_toll(tile.x, tile.z, tile.level)
    }

    pub(crate) fn loc_def(&self, loc_id: usize) -> Option<LocDef> {
        self.world.loc_def(loc_id)
    }

    pub(crate) fn in_f2p(&self, tile_x: i32, tile_z: i32) -> bool {
        self.world.in_f2p(tile_x, tile_z)
    }

    /// Remove one door's wall footprint from a scratch map.
    pub(crate) fn unpaint_into(&self, map: &mut CollisionMap, loc: u16, shape: u8, angle: u8, at: Tile) {
        self.world.unpaint_into(map, loc, shape, angle, at.x, at.z, at.level);
    }
}
