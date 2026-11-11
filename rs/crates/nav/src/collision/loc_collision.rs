//! Painted collision map: walls, loc footprints and single-step checks.

use std::collections::HashMap;

use super::flags::*;

/// Minimal loc-type view the map loader needs (JSON parsing stays in gamedata).
#[derive(Debug, Clone, Copy)]
pub struct LocDef {
    pub blockwalk: bool,
    pub blockrange: bool,
    pub length: i32,
    pub width: i32,
    pub active: i32,
    pub has_ops: bool,
    /// 1-based op index of the "open" action, when the loc can be opened.
    pub open_op: Option<u8>,
    /// The "open" action charges a toll (Al-Kharid gate): never routed
    /// through unless the session explicitly allows paid paths.
    pub toll: bool,
}

/// 8x8-tile zones keyed like the server, flags painted by locs and walls.
#[derive(Debug, Default, Clone)]
pub struct CollisionMap {
    zones: HashMap<u32, [u32; 64]>,
}

impl CollisionMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn zone_mut(&mut self, tile_x: i32, tile_z: i32, level: i32) -> &mut [u32; 64] {
        self.zones.entry(zone_index(tile_x, tile_z, level)).or_insert([0; 64])
    }

    pub fn get(&self, tile_x: i32, tile_z: i32, level: i32) -> u32 {
        match self.zones.get(&zone_index(tile_x, tile_z, level)) {
            Some(zone) => zone[tile_index(tile_x, tile_z)],
            None => NULL,
        }
    }

    /// True when this tile belongs to a map zone actually loaded.
    pub fn has(&self, tile_x: i32, tile_z: i32, level: i32) -> bool {
        self.zones.contains_key(&zone_index(tile_x, tile_z, level))
    }

    /// Zone flags keyed like the server. Offline tools walk them with
    /// unsigned 8x8 bases, exactly like the packed format iteration.
    pub fn raw_zones(&self) -> &HashMap<u32, [u32; 64]> {
        &self.zones
    }

    /// Every tile carrying any flag, used to read a painted door footprint.
    pub fn flagged_tiles(&self) -> Vec<(i32, i32, i32)> {
        // Inverse of zone_index, with sign extension for negative squares.
        fn zone_base(field: u32) -> i32 {
            let signed = field as i32;
            (if signed >= 0x400 { signed - 0x800 } else { signed }) << 3
        }
        let mut tiles = Vec::new();
        for (zone_key, zone) in &self.zones {
            let base_x = zone_base(zone_key & 0x7FF);
            let base_z = zone_base((zone_key >> 11) & 0x7FF);
            let level = ((zone_key >> 22) & 0x3) as i32;
            for (slot, flags) in zone.iter().enumerate() {
                if *flags != OPEN {
                    let slot = slot as i32;
                    tiles.push((base_x + (slot & 0x7), base_z + ((slot >> 3) & 0x7), level));
                }
            }
        }
        tiles
    }

    pub fn is_flagged(&self, tile_x: i32, tile_z: i32, level: i32, mask: u32) -> bool {
        self.get(tile_x, tile_z, level) & mask != OPEN
    }

    pub fn set(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        self.zone_mut(tile_x, tile_z, level)[slot] = mask & MASK32;
    }

    pub fn add(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        let zone = self.zone_mut(tile_x, tile_z, level);
        zone[slot] = (zone[slot] | mask) & MASK32;
    }

    pub fn remove(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32) {
        let slot = tile_index(tile_x, tile_z);
        let zone = self.zone_mut(tile_x, tile_z, level);
        zone[slot] = (zone[slot] & !mask) & MASK32;
    }

    pub fn change_floor(&mut self, tile_x: i32, tile_z: i32, level: i32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, FLOOR);
        } else {
            self.remove(tile_x, tile_z, level, FLOOR);
        }
    }

    pub fn change_roof(&mut self, tile_x: i32, tile_z: i32, level: i32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, ROOF);
        } else {
            self.remove(tile_x, tile_z, level, ROOF);
        }
    }

    pub fn change_npc(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, NPC_OCC, add);
    }

    pub fn change_block(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, BLOCK_NPC_AND_PLAYERS, add);
    }

    pub fn change_player_occ(&mut self, tile_x: i32, tile_z: i32, level: i32, size: i32, add: bool) {
        self.square(tile_x, tile_z, level, size, PLAYER_OCC, add);
    }

    fn square(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        size: i32,
        mask: u32,
        add: bool,
    ) {
        for index in 0..size * size {
            let dest_x = tile_x + index % size;
            let dest_z = tile_z + index / size;
            if add {
                self.add(dest_x, dest_z, level, mask);
            } else {
                self.remove(dest_x, dest_z, level, mask);
            }
        }
    }

    pub fn change_loc(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        width: i32,
        length: i32,
        blockrange: bool,
        add: bool,
    ) {
        let mut mask = LOC;
        if blockrange {
            mask |= LOC_PROJ_BLOCKER;
        }
        for index in 0..width * length {
            let dest_x = tile_x + index % width;
            let dest_z = tile_z + index / width;
            if add {
                self.add(dest_x, dest_z, level, mask);
            } else {
                self.remove(dest_x, dest_z, level, mask);
            }
        }
    }

    pub fn change_wall(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        shape: u8,
        blockrange: bool,
        add: bool,
    ) {
        if shape == WALL_STRAIGHT {
            self.wall_straight(tile_x, tile_z, level, angle, blockrange, add);
        } else if shape == WALL_DIAGONAL_CORNER || shape == WALL_SQUARE_CORNER {
            self.wall_corner(tile_x, tile_z, level, angle, blockrange, add);
        } else if shape == WALL_L {
            self.wall_l(tile_x, tile_z, level, angle, blockrange, add);
        }
    }

    fn wall_mask(normal: u32, projectile: u32, blockrange: bool) -> u32 {
        if blockrange { projectile } else { normal }
    }

    fn wall_straight(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let west = Self::wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange);
        let east = Self::wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange);
        let north = Self::wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange);
        let south = Self::wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange);
        match angle {
            ANGLE_WEST => self.wall_pair(tile_x, tile_z, level, west, tile_x - 1, tile_z, level, east, add),
            ANGLE_NORTH => self.wall_pair(tile_x, tile_z, level, north, tile_x, tile_z + 1, level, south, add),
            ANGLE_EAST => self.wall_pair(tile_x, tile_z, level, east, tile_x + 1, tile_z, level, west, add),
            ANGLE_SOUTH => self.wall_pair(tile_x, tile_z, level, south, tile_x, tile_z - 1, level, north, add),
            _ => {}
        }
        if blockrange {
            self.wall_straight(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_corner(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let north_west = Self::wall_mask(WALL_NORTH_WEST, WALL_NORTH_WEST_PROJ, blockrange);
        let south_east = Self::wall_mask(WALL_SOUTH_EAST, WALL_SOUTH_EAST_PROJ, blockrange);
        let north_east = Self::wall_mask(WALL_NORTH_EAST, WALL_NORTH_EAST_PROJ, blockrange);
        let south_west = Self::wall_mask(WALL_SOUTH_WEST, WALL_SOUTH_WEST_PROJ, blockrange);
        match angle {
            ANGLE_WEST => self.wall_pair(
                tile_x, tile_z, level, north_west, tile_x - 1, tile_z + 1, level, south_east, add,
            ),
            ANGLE_NORTH => self.wall_pair(
                tile_x, tile_z, level, north_east, tile_x + 1, tile_z + 1, level, south_west, add,
            ),
            ANGLE_EAST => self.wall_pair(
                tile_x, tile_z, level, south_east, tile_x + 1, tile_z - 1, level, north_west, add,
            ),
            ANGLE_SOUTH => self.wall_pair(
                tile_x, tile_z, level, south_west, tile_x - 1, tile_z - 1, level, north_east, add,
            ),
            _ => {}
        }
        if blockrange {
            self.wall_corner(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_l(
        &mut self,
        tile_x: i32,
        tile_z: i32,
        level: i32,
        angle: u8,
        blockrange: bool,
        add: bool,
    ) {
        let west = Self::wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange);
        let east = Self::wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange);
        let north = Self::wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange);
        let south = Self::wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange);
        match angle {
            ANGLE_WEST => {
                self.wall_single(tile_x, tile_z, level, north | west, add);
                self.wall_single(tile_x - 1, tile_z, level, east, add);
                self.wall_single(tile_x, tile_z + 1, level, south, add);
            }
            ANGLE_NORTH => {
                self.wall_single(tile_x, tile_z, level, north | east, add);
                self.wall_single(tile_x + 1, tile_z, level, west, add);
                self.wall_single(tile_x, tile_z + 1, level, south, add);
            }
            ANGLE_EAST => {
                self.wall_single(tile_x, tile_z, level, south | east, add);
                self.wall_single(tile_x + 1, tile_z, level, west, add);
                self.wall_single(tile_x, tile_z - 1, level, north, add);
            }
            ANGLE_SOUTH => {
                self.wall_single(tile_x, tile_z, level, south | west, add);
                self.wall_single(tile_x, tile_z - 1, level, north, add);
                self.wall_single(tile_x - 1, tile_z, level, east, add);
            }
            _ => {}
        }
        if blockrange {
            self.wall_l(tile_x, tile_z, level, angle, false, add);
        }
    }

    fn wall_pair(
        &mut self,
        first_x: i32,
        first_z: i32,
        first_level: i32,
        first_mask: u32,
        second_x: i32,
        second_z: i32,
        second_level: i32,
        second_mask: u32,
        add: bool,
    ) {
        self.wall_single(first_x, first_z, first_level, first_mask, add);
        self.wall_single(second_x, second_z, second_level, second_mask, add);
    }

    fn wall_single(&mut self, tile_x: i32, tile_z: i32, level: i32, mask: u32, add: bool) {
        if add {
            self.add(tile_x, tile_z, level, mask);
        } else {
            self.remove(tile_x, tile_z, level, mask);
        }
    }
}

pub fn layer_of(shape: u8) -> u8 {
    if shape <= 3 {
        LAYER_WALL
    } else if (4..=8).contains(&shape) {
        LAYER_WALL_DECOR
    } else if shape == 22 {
        LAYER_GROUND_DECOR
    } else {
        LAYER_GROUND
    }
}

pub fn change_loc_collision(
    collision: &mut CollisionMap,
    shape: u8,
    angle: u8,
    blockrange: bool,
    length: i32,
    width: i32,
    active: i32,
    tile_x: i32,
    tile_z: i32,
    level: i32,
    add: bool,
) {
    match layer_of(shape) {
        LAYER_WALL => collision.change_wall(tile_x, tile_z, level, angle, shape, blockrange, add),
        LAYER_GROUND => {
            if angle == ANGLE_NORTH || angle == ANGLE_SOUTH {
                collision.change_loc(tile_x, tile_z, level, length, width, blockrange, add);
            } else {
                collision.change_loc(tile_x, tile_z, level, width, length, blockrange, add);
            }
        }
        LAYER_GROUND_DECOR => {
            if active == 1 {
                collision.change_floor(tile_x, tile_z, level, add);
            }
        }
        _ => {}
    }
}

/// True when a size-1 mover can step one tile straight (diagonals rejected:
/// only the four cardinals are single steps).
pub fn step_open(
    collision: &CollisionMap,
    tile_x: i32,
    tile_z: i32,
    level: i32,
    dest_dx: i32,
    dest_dz: i32,
) -> bool {
    let here = collision.get(tile_x, tile_z, level);
    let there = collision.get(tile_x + dest_dx, tile_z + dest_dz, level);
    if !tile_walkable(here) || !tile_walkable(there) {
        return false;
    }
    if dest_dx == 0 && dest_dz == 1 {
        return here & WALL_NORTH == 0 && there & WALL_SOUTH == 0;
    }
    if dest_dx == 0 && dest_dz == -1 {
        return here & WALL_SOUTH == 0 && there & WALL_NORTH == 0;
    }
    if dest_dx == 1 && dest_dz == 0 {
        return here & WALL_EAST == 0 && there & WALL_WEST == 0;
    }
    if dest_dx == -1 && dest_dz == 0 {
        return here & WALL_WEST == 0 && there & WALL_EAST == 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_pair_blocks_both_sides() {
        let mut collision = CollisionMap::new();
        collision.change_wall(10, 10, 0, ANGLE_WEST, WALL_STRAIGHT, false, true);
        assert!(collision.is_flagged(10, 10, 0, WALL_WEST));
        assert!(collision.is_flagged(9, 10, 0, WALL_EAST));
        assert!(!step_open(&collision, 10, 10, 0, -1, 0));
        assert!(step_open(&collision, 10, 10, 0, 1, 0));
        collision.change_wall(10, 10, 0, ANGLE_WEST, WALL_STRAIGHT, false, false);
        assert!(step_open(&collision, 10, 10, 0, -1, 0));
    }

    #[test]
    fn unknown_tiles_read_null_and_count_as_blocked() {
        // Missing zones read NULL (0x7FFFFFFF), which has
        // every flag bit set, so unknown tiles are not walkable.
        let collision = CollisionMap::new();
        assert_eq!(collision.get(100, 100, 0), NULL);
        assert!(!collision.has(100, 100, 0));
        assert!(!tile_walkable(NULL));
    }

    #[test]
    fn loc_footprint_masks_ground_layer() {
        let mut collision = CollisionMap::new();
        change_loc_collision(&mut collision, 10, ANGLE_NORTH, false, 2, 3, 0, 5, 5, 0, true);
        assert!(collision.is_flagged(6, 5, 0, LOC));
        assert!(collision.is_flagged(5, 6, 0, LOC));
        assert!(collision.has(6, 5, 0));
        assert!(!collision.has(8, 5, 0));
    }

    /// Furniture paints exactly like the server: centrepiece tables and
    /// chairs (shape 10, blockwalk) mask the ground layer, and ground
    /// decor only paints when active (the server `active === 1` gate).
    /// A table that did not block would read OPEN here.
    #[test]
    fn furniture_blocks_like_the_server() {
        let mut collision = CollisionMap::new();
        // Round table 598 and chair 1088: 1x1 centrepieces, blockwalk.
        change_loc_collision(&mut collision, 10, 3, false, 1, 1, 1, 20, 20, 0, true);
        assert!(collision.is_flagged(20, 20, 0, LOC), "table footprint must block");
        assert!(!tile_walkable(collision.get(20, 20, 0) | crate::collision::FLOOR | crate::collision::BLOCK_NPC_AND_PLAYERS), "blocked tile reads like the server WALK_BLOCKED");
        // Ground decor with active 0 (rugs, grass): no paint at all.
        let mut decor = CollisionMap::new();
        change_loc_collision(&mut decor, 22, 0, false, 1, 1, 0, 30, 30, 0, true);
        assert!(!decor.has(30, 30, 0), "inactive decor paints nothing, not even a zone");
        // Same decor active: floor flag, like the server changeFloor.
        change_loc_collision(&mut decor, 22, 0, false, 1, 1, 1, 31, 31, 0, true);
        assert!(decor.is_flagged(31, 31, 0, crate::collision::FLOOR), "active decor paints floor");
    }
}
