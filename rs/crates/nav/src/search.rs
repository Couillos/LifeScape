#![forbid(unsafe_code)]
//! The door-aware view of the world that every tile search reads
//! (lot N1, 2026-11-24).
//!
//! One implementation for `plan::dijkstra`, `plan::prepare_edges`,
//! `costs::plan_costs_capped` and the sealed-pocket flood (`pocket`):
//! they each held a copy of these methods and a copy of their four
//! per-search tables. Every method reads through `Reader`, caches per
//! search, and computes exactly what the copies did.
//!
//! Two door rules exist. A planning search swings the doors it may use
//! (openable, untolled or paid, requirement tags in hand, not blocked
//! tile by tile). The permissive view (`Search::permissive`, pockets)
//! swings every door with an open op that is not blocked tile by tile:
//! the widest walk any search could take.

use crate::bfs::can_step;
use crate::collision::CollisionMap;
use crate::plan::{NavOptions, Tile, U64Map};
use crate::reader::Reader;
use crate::static_world::StaticWorld;
use crate::tables::Tables;

pub(crate) struct Search<'a> {
    pub(crate) world: &'a StaticWorld,
    pub(crate) reader: Reader<'a>,
    pub(crate) tables: &'a Tables,
    pub(crate) options: &'a NavOptions,
    pub(crate) ignore_requires: bool,
    pub(crate) allow_paid: bool,
    pub(crate) ignore_f2p: bool,
    /// Pocket flood: every openable door counts, tolls and requirements
    /// ignored, no F2P bound.
    permissive: bool,
    base_flags: U64Map<u32>,
    open_flags: U64Map<u32>,
    doors: U64Map<Option<(u16, u8, u8)>>,
    /// F2P verdict per 8x8 zone (`in_f2p` takes the world lock: it ran
    /// for every neighbour of every visit).
    f2p_zones: U64Map<bool>,
}

impl<'a> Search<'a> {
    pub(crate) fn new(
        world: &'a StaticWorld,
        tables: &'a Tables,
        options: &'a NavOptions,
        ignore_requires: bool,
        allow_paid: bool,
        ignore_f2p: bool,
    ) -> Search<'a> {
        Search {
            world,
            reader: Reader::new(world),
            tables,
            options,
            ignore_requires,
            allow_paid,
            ignore_f2p,
            permissive: false,
            base_flags: U64Map::default(),
            open_flags: U64Map::default(),
            doors: U64Map::default(),
            f2p_zones: U64Map::default(),
        }
    }

    /// The widest view (pockets): `options` only fills the unused slots.
    pub(crate) fn permissive(world: &'a StaticWorld, tables: &'a Tables, options: &'a NavOptions) -> Search<'a> {
        let mut search = Search::new(world, tables, options, true, true, true);
        search.permissive = true;
        search
    }

    pub(crate) fn in_bounds(&mut self, tile: Tile) -> bool {
        if !self.options.f2p_only || self.ignore_f2p {
            return true;
        }
        let zone = Tile { x: tile.x >> 3, z: tile.z >> 3, level: 0 }.key();
        if let Some(inside) = self.f2p_zones.get(&zone) {
            return *inside;
        }
        let inside = self.reader.in_f2p(tile.x, tile.z);
        self.f2p_zones.insert(zone, inside);
        inside
    }

    pub(crate) fn base(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.base_flags.get(&tile.key()) {
            return *flags;
        }
        let flags = self.reader.flags(tile);
        self.base_flags.insert(tile.key(), flags);
        flags
    }

    pub(crate) fn door_at(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        if let Some(found) = self.doors.get(&tile.key()) {
            return *found;
        }
        let found = self.reader.door_at(tile);
        self.doors.insert(tile.key(), found);
        found
    }

    /// A door this search may swing: openable, untolled (or paid), with
    /// its requirement tags in hand, and not hand-blocked tile by tile.
    /// The permissive view keeps the openable, not-blocked ones.
    pub(crate) fn usable_door(&mut self, tile: Tile) -> Option<(u16, u8, u8)> {
        let (id, shape, angle) = self.door_at(tile)?;
        if self.tables.is_blocked(id, tile) {
            return None;
        }
        let def = self.reader.loc_def(id as usize)?;
        def.open_op?;
        if self.permissive {
            return Some((id, shape, angle));
        }
        if self.reader.is_toll(tile) && !self.allow_paid {
            return None;
        }
        if !self.ignore_requires && !self.tables.missing(id, tile, &self.options.satisfied).is_empty() {
            return None;
        }
        Some((id, shape, angle))
    }

    /// Collision with usable doors swung open: the exact wall removal
    /// over the tiles it touches, never a blanket OPEN. Wall locs paint
    /// shared edges onto their neighbours, so the scratch map carries
    /// the 3x3 around the tile and every usable door inside unpaints
    /// into it before the centre tile is read. 3x3 is exact: paint
    /// reaches at most the neighbour tiles, so any door touching the
    /// centre always shows one of its tiles inside the window.
    pub(crate) fn open(&mut self, tile: Tile) -> u32 {
        if let Some(flags) = self.open_flags.get(&tile.key()) {
            return *flags;
        }
        let mut doors = Vec::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                if let Some(found) = self.usable_door(near) {
                    doors.push((near, found));
                }
            }
        }
        let flags = if doors.is_empty() {
            self.base(tile)
        } else {
            let mut scratch = CollisionMap::new();
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let near = Tile { x: tile.x + dx, z: tile.z + dz, level: tile.level };
                    scratch.set(near.x, near.z, near.level, self.base(near));
                }
            }
            for (near, (id, shape, angle)) in doors {
                self.reader.unpaint_into(&mut scratch, id, shape, angle, near);
            }
            scratch.get(tile.x, tile.z, tile.level)
        };
        self.open_flags.insert(tile.key(), flags);
        flags
    }

    /// Walk step in the planning view (usable doors read open).
    pub(crate) fn step_open(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.open(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }

    /// Walk step with every door shut.
    pub(crate) fn step_closed(&mut self, from: Tile, to: Tile) -> bool {
        let level = from.level;
        let mut step = |x: i32, z: i32| self.base(Tile { x, z, level });
        can_step(&mut step, from.x, from.z, to.x, to.z)
    }
}
