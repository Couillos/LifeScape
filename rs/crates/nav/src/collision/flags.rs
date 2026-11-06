//! Collision flag constants and tile index helpers, same values as the game server.

pub const OPEN: u32 = 0x0;
pub const WALL_NORTH_WEST: u32 = 0x1;
pub const WALL_NORTH: u32 = 0x2;
pub const WALL_NORTH_EAST: u32 = 0x4;
pub const WALL_EAST: u32 = 0x8;
pub const WALL_SOUTH_EAST: u32 = 0x10;
pub const WALL_SOUTH: u32 = 0x20;
pub const WALL_SOUTH_WEST: u32 = 0x40;
pub const WALL_WEST: u32 = 0x80;
pub const LOC: u32 = 0x100;
pub const WALL_NORTH_WEST_PROJ: u32 = 0x200;
pub const WALL_NORTH_PROJ: u32 = 0x400;
pub const WALL_NORTH_EAST_PROJ: u32 = 0x800;
pub const WALL_EAST_PROJ: u32 = 0x1000;
pub const WALL_SOUTH_EAST_PROJ: u32 = 0x2000;
pub const WALL_SOUTH_PROJ: u32 = 0x4000;
pub const WALL_SOUTH_WEST_PROJ: u32 = 0x8000;
pub const WALL_WEST_PROJ: u32 = 0x10000;
pub const LOC_PROJ_BLOCKER: u32 = 0x20000;
pub const FLOOR_DECORATION: u32 = 0x40000;
pub const NPC_OCC: u32 = 0x80000;
pub const BLOCK_NPC_AND_PLAYERS: u32 = 0x100000;
pub const FLOOR: u32 = 0x200000;
pub const PLAYER_OCC: u32 = 0x400000;
pub const ROOF: u32 = 0x80000000;
pub const FLOOR_BLOCKED: u32 = 0x240000;
pub const WALK_BLOCKED: u32 = 0x240100;
pub const BLOCK_WEST: u32 = 0x240108;
pub const BLOCK_EAST: u32 = 0x240180;
pub const BLOCK_SOUTH: u32 = 0x240102;
pub const BLOCK_NORTH: u32 = 0x240120;
pub const BLOCK_SOUTH_WEST: u32 = 0x24010E;
pub const BLOCK_SOUTH_EAST: u32 = 0x240183;
pub const BLOCK_NORTH_WEST: u32 = 0x240138;
pub const BLOCK_NORTH_EAST: u32 = 0x2401E0;

/// Sentinel for tiles in map squares never loaded.
pub const NULL: u32 = 0x7FFFFFFF;
pub(crate) const MASK32: u32 = 0xFFFFFFFF;

pub const WALL_STRAIGHT: u8 = 0;
pub const WALL_DIAGONAL_CORNER: u8 = 1;
pub const WALL_L: u8 = 2;
pub const WALL_SQUARE_CORNER: u8 = 3;

pub const ANGLE_WEST: u8 = 0;
pub const ANGLE_NORTH: u8 = 1;
pub const ANGLE_EAST: u8 = 2;
pub const ANGLE_SOUTH: u8 = 3;

pub const LAYER_WALL: u8 = 0;
pub const LAYER_WALL_DECOR: u8 = 1;
pub const LAYER_GROUND: u8 = 2;
pub const LAYER_GROUND_DECOR: u8 = 3;

pub const BLOCK_MAP_SQUARE: u8 = 0x1;
pub const LINK_BELOW: u8 = 0x2;
pub const REMOVE_ROOFS: u8 = 0x4;

pub const MAP_SIZE: usize = 64;
pub const LEVELS: usize = 4;
pub const LANDS: usize = MAP_SIZE * MAP_SIZE * LEVELS;

/// True unless every direction is blocked (roof/floor flags alone are fine).
pub fn tile_walkable(flags: u32) -> bool {
    !((flags & BLOCK_NORTH) != 0
        && (flags & BLOCK_EAST) != 0
        && (flags & BLOCK_SOUTH) != 0
        && (flags & BLOCK_WEST) != 0)
}

pub fn zone_index(tile_x: i32, tile_z: i32, level: i32) -> u32 {
    (((tile_x >> 3) & 0x7FF) | (((tile_z >> 3) & 0x7FF) << 11) | ((level & 0x3) << 22)) as u32
}

pub fn tile_index(tile_x: i32, tile_z: i32) -> usize {
    ((tile_x & 0x7) | ((tile_z & 0x7) << 3)) as usize
}

pub fn pack_land(land_x: i32, land_z: i32, level: i32) -> usize {
    ((land_z & 0x3F) | ((land_x & 0x3F) << 6) | ((level & 0x3) << 12)) as usize
}
