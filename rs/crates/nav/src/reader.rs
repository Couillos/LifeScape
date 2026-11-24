#![forbid(unsafe_code)]
//! The one world reader of the navigation searches (lot N1, 2026-11-24).
//!
//! `plan::dijkstra`, `costs::plan_costs_capped`, `fast_hop::short_hop`
//! and the sealed-pocket flood used to call `StaticWorld` through four
//! private copies of the same reads (flags, door, F2P zone, toll). They
//! now go through this type: the seam where the dense grid of lot N2
//! lives, so one change speeds up every search. A reader is made once per
//! search and is cheap to copy around by reference.

use std::sync::Arc;

use crate::collision::{CollisionMap, LocDef};
use crate::grid::{Grid, Square};
use crate::plan::Tile;
use crate::static_world::StaticWorld;

/// Static reads of one search. Holds the world's dense grid as it stood
/// when the search began (one `Arc` clone), so a tile read is an index
/// and a load, never a lock. A tile the grid does not index, or a pack
/// square that cannot be read, falls back to the paged reader.
pub(crate) struct Reader<'a> {
    world: &'a StaticWorld,
    grid: Arc<Grid>,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(world: &'a StaticWorld) -> Reader<'a> {
        Reader { world, grid: world.grid() }
    }

    fn square(&self, tile: Tile) -> Option<&Square> {
        self.grid.square(self.world, tile.x, tile.z, tile.level)
    }

    /// Collision flags of one tile (`StaticWorld::get`).
    pub(crate) fn flags(&self, tile: Tile) -> u32 {
        match self.square(tile) {
            Some(square) => square.flags(tile.x, tile.z),
            None => self.world.get_paged(tile.x, tile.z, tile.level),
        }
    }

    /// False only when no closed door is recorded on this tile.
    pub(crate) fn has_door(&self, tile: Tile) -> bool {
        match self.square(tile) {
            Some(square) => square.has_door(tile.x, tile.z),
            None => true,
        }
    }

    /// False only when no door stands on this tile or its 8 neighbours.
    pub(crate) fn near_door(&self, tile: Tile) -> bool {
        match self.square(tile) {
            Some(square) => square.near_door(tile.x, tile.z),
            None => true,
        }
    }

    /// Closed door recorded on this tile, if any.
    pub(crate) fn door_at(&self, tile: Tile) -> Option<(u16, u8, u8)> {
        if !self.has_door(tile) {
            return None;
        }
        self.world.door_at(tile.x, tile.z, tile.level)
    }

    pub(crate) fn is_toll(&self, tile: Tile) -> bool {
        self.world.is_toll(tile.x, tile.z, tile.level)
    }

    pub(crate) fn loc_def(&self, loc_id: usize) -> Option<LocDef> {
        self.world.loc_def(loc_id)
    }

    /// Free-to-play ground (level ignored, like the engine's check).
    pub(crate) fn in_f2p(&self, tile: Tile) -> bool {
        match self.square(tile) {
            Some(square) => square.in_f2p(tile.x, tile.z),
            None => self.world.in_f2p(tile.x, tile.z),
        }
    }

    /// Remove one door's wall footprint from a scratch map.
    pub(crate) fn unpaint_into(&self, map: &mut CollisionMap, loc: u16, shape: u8, angle: u8, at: Tile) {
        self.world.unpaint_into(map, loc, shape, angle, at.x, at.z, at.level);
    }
}
