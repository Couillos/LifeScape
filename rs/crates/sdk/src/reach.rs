//! Reach checks: when the bot is "close enough" without stepping on the tile.
//!
//! Exact port of lifescape/reach.py (server ReachStrategy, size 1 only:
//! the bot is always a solo player).

use crate::collision;

pub const SHAPE_TILE: i32 = -1;
pub const SHAPE_NPC: i32 = -2;

pub const ANGLE_WEST: i32 = 0;
pub const ANGLE_NORTH: i32 = 1;
pub const ANGLE_EAST: i32 = 2;
pub const ANGLE_SOUTH: i32 = 3;

pub const BLOCK_NORTH: u32 = 0x1;
pub const BLOCK_EAST: u32 = 0x2;
pub const BLOCK_SOUTH: u32 = 0x4;
pub const BLOCK_WEST: u32 = 0x8;

pub fn rotate(angle: i32, unrotated: i32, rotated: i32) -> i32 {
    if angle & 0x1 != 0 {
        rotated
    } else {
        unrotated
    }
}

pub fn rotate_flags(angle: i32, block_access: u32) -> u32 {
    if angle == 0 {
        return block_access;
    }
    (((block_access << angle) & 0xF) | (block_access >> (4 - angle))) & 0xF
}

pub fn collides(
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    src_width: i32,
    src_height: i32,
    dest_width: i32,
    dest_height: i32,
) -> bool {
    if src_x >= dest_x + dest_width || src_x + src_width <= dest_x {
        return false;
    }
    src_z < dest_z + dest_height && dest_z < src_height + src_z
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExitStrategy {
    None,
    Wall,
    Decor,
    Rect,
    Exclusive,
}

fn exit_strategy(shape: i32) -> ExitStrategy {
    if shape == SHAPE_NPC {
        return ExitStrategy::Exclusive;
    }
    if shape == SHAPE_TILE {
        return ExitStrategy::None;
    }
    if (0..=3).contains(&shape) || shape == 9 {
        return ExitStrategy::Wall;
    }
    if shape < 9 {
        return ExitStrategy::Decor;
    }
    if shape == 10 || shape == 11 || shape == 22 {
        return ExitStrategy::Rect;
    }
    ExitStrategy::None
}

pub fn altered_rotation(angle: i32, shape: i32) -> i32 {
    if shape == 7 {
        (angle + 2) & 0x3
    } else {
        angle
    }
}

/// True when (src_x, src_z) suffices to interact with the target.
/// get_flag(x, z, level) reads one collision tile.
pub fn reached(
    get_flag: &dyn Fn(i32, i32, i32) -> u32,
    level: i32,
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    dest_width: i32,
    dest_height: i32,
    angle: i32,
    shape: i32,
    block_access: u32,
) -> bool {
    let strategy = exit_strategy(shape);
    if strategy != ExitStrategy::Exclusive && src_x == dest_x && src_z == dest_z {
        return true;
    }
    match strategy {
        ExitStrategy::Wall => {
            reach_wall(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle)
        }
        ExitStrategy::Decor => {
            reach_decor(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle)
        }
        ExitStrategy::Rect => reach_rect(
            get_flag,
            level,
            src_x,
            src_z,
            dest_x,
            dest_z,
            dest_width,
            dest_height,
            angle,
            block_access,
        ),
        ExitStrategy::Exclusive => {
            !collides(src_x, src_z, dest_x, dest_z, 1, 1, dest_width, dest_height)
                && reach_rect(
                    get_flag,
                    level,
                    src_x,
                    src_z,
                    dest_x,
                    dest_z,
                    dest_width,
                    dest_height,
                    angle,
                    block_access,
                )
        }
        ExitStrategy::None => false,
    }
}

fn reach_rect(
    get_flag: &dyn Fn(i32, i32, i32) -> u32,
    level: i32,
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    dest_width: i32,
    dest_height: i32,
    angle: i32,
    block_access: u32,
) -> bool {
    let width = rotate(angle, dest_width, dest_height);
    let height = rotate(angle, dest_height, dest_width);
    let access = rotate_flags(angle, block_access);
    if collides(src_x, src_z, dest_x, dest_z, 1, 1, width, height) {
        return true;
    }
    reach_rect_side(get_flag, level, src_x, src_z, dest_x, dest_z, width, height, access)
}

fn reach_rect_side(
    get_flag: &dyn Fn(i32, i32, i32) -> u32,
    level: i32,
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    width: i32,
    height: i32,
    access: u32,
) -> bool {
    let east = dest_x + width - 1;
    let north = dest_z + height - 1;
    if src_x == dest_x - 1
        && dest_z <= src_z
        && src_z <= north
        && (get_flag(src_x, src_z, level) & collision::WALL_EAST) == collision::OPEN
        && (access & BLOCK_WEST) == 0
    {
        return true;
    }
    if src_x == east + 1
        && dest_z <= src_z
        && src_z <= north
        && (get_flag(src_x, src_z, level) & collision::WALL_WEST) == collision::OPEN
        && (access & BLOCK_EAST) == 0
    {
        return true;
    }
    if src_z + 1 == dest_z
        && dest_x <= src_x
        && src_x <= east
        && (get_flag(src_x, src_z, level) & collision::WALL_NORTH) == collision::OPEN
        && (access & BLOCK_SOUTH) == 0
    {
        return true;
    }
    src_z == north + 1
        && dest_x <= src_x
        && src_x <= east
        && (get_flag(src_x, src_z, level) & collision::WALL_SOUTH) == collision::OPEN
        && (access & BLOCK_NORTH) == 0
}

fn reach_wall(
    get_flag: &dyn Fn(i32, i32, i32) -> u32,
    level: i32,
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    shape: i32,
    angle: i32,
) -> bool {
    if src_x == dest_x && src_z == dest_z {
        return true;
    }
    let tile = get_flag(src_x, src_z, level);
    if shape == 0 {
        if angle == ANGLE_WEST {
            return (src_x == dest_x - 1 && src_z == dest_z)
                || (src_x == dest_x
                    && src_z == dest_z + 1
                    && (tile & collision::BLOCK_NORTH) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::BLOCK_SOUTH) == collision::OPEN);
        }
        if angle == ANGLE_NORTH {
            return (src_x == dest_x && src_z == dest_z + 1)
                || (src_x == dest_x - 1
                    && src_z == dest_z
                    && (tile & collision::BLOCK_WEST) == collision::OPEN)
                || (src_x == dest_x + 1
                    && src_z == dest_z
                    && (tile & collision::BLOCK_EAST) == collision::OPEN);
        }
        if angle == ANGLE_EAST {
            return (src_x == dest_x + 1 && src_z == dest_z)
                || (src_x == dest_x
                    && src_z == dest_z + 1
                    && (tile & collision::BLOCK_NORTH) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::BLOCK_SOUTH) == collision::OPEN);
        }
        return (src_x == dest_x && src_z == dest_z - 1)
            || (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::BLOCK_WEST) == collision::OPEN)
            || (src_x == dest_x + 1
                && src_z == dest_z
                && (tile & collision::BLOCK_EAST) == collision::OPEN);
    }
    if shape == 2 {
        if angle == ANGLE_WEST {
            return (src_x == dest_x - 1 && src_z == dest_z)
                || (src_x == dest_x && src_z == dest_z + 1)
                || (src_x == dest_x + 1
                    && src_z == dest_z
                    && (tile & collision::BLOCK_EAST) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::BLOCK_SOUTH) == collision::OPEN);
        }
        if angle == ANGLE_NORTH {
            return (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::BLOCK_WEST) == collision::OPEN)
                || (src_x == dest_x && src_z == dest_z + 1)
                || (src_x == dest_x + 1 && src_z == dest_z)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::BLOCK_SOUTH) == collision::OPEN);
        }
        if angle == ANGLE_EAST {
            return (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::BLOCK_WEST) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z + 1
                    && (tile & collision::BLOCK_NORTH) == collision::OPEN)
                || (src_x == dest_x + 1 && src_z == dest_z)
                || (src_x == dest_x && src_z == dest_z - 1);
        }
        return (src_x == dest_x - 1 && src_z == dest_z)
            || (src_x == dest_x
                && src_z == dest_z + 1
                && (tile & collision::BLOCK_NORTH) == collision::OPEN)
            || (src_x == dest_x + 1
                && src_z == dest_z
                && (tile & collision::BLOCK_EAST) == collision::OPEN)
            || (src_x == dest_x && src_z == dest_z - 1);
    }
    if shape == 1 {
        return (src_x == dest_x
            && src_z == dest_z + 1
            && (tile & collision::WALL_SOUTH) == collision::OPEN)
            || (src_x == dest_x
                && src_z == dest_z - 1
                && (tile & collision::WALL_NORTH) == collision::OPEN)
            || (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::WALL_EAST) == collision::OPEN)
            || (src_x == dest_x + 1
                && src_z == dest_z
                && (tile & collision::WALL_WEST) == collision::OPEN);
    }
    false
}

fn reach_decor(
    get_flag: &dyn Fn(i32, i32, i32) -> u32,
    level: i32,
    src_x: i32,
    src_z: i32,
    dest_x: i32,
    dest_z: i32,
    shape: i32,
    angle: i32,
) -> bool {
    if src_x == dest_x && src_z == dest_z {
        return true;
    }
    let tile = get_flag(src_x, src_z, level);
    if shape == 6 || shape == 7 {
        let rotation = altered_rotation(angle, shape);
        if rotation == ANGLE_WEST {
            return (src_x == dest_x + 1
                && src_z == dest_z
                && (tile & collision::WALL_WEST) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::WALL_NORTH) == collision::OPEN);
        }
        if rotation == ANGLE_NORTH {
            return (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::WALL_EAST) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z - 1
                    && (tile & collision::WALL_NORTH) == collision::OPEN);
        }
        if rotation == ANGLE_EAST {
            return (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::WALL_EAST) == collision::OPEN)
                || (src_x == dest_x
                    && src_z == dest_z + 1
                    && (tile & collision::WALL_SOUTH) == collision::OPEN);
        }
        return (src_x == dest_x + 1
            && src_z == dest_z
            && (tile & collision::WALL_WEST) == collision::OPEN)
            || (src_x == dest_x
                && src_z == dest_z + 1
                && (tile & collision::WALL_SOUTH) == collision::OPEN);
    }
    if shape == 8 {
        return (src_x == dest_x
            && src_z == dest_z + 1
            && (tile & collision::WALL_SOUTH) == collision::OPEN)
            || (src_x == dest_x
                && src_z == dest_z - 1
                && (tile & collision::WALL_NORTH) == collision::OPEN)
            || (src_x == dest_x - 1
                && src_z == dest_z
                && (tile & collision::WALL_EAST) == collision::OPEN)
            || (src_x == dest_x + 1
                && src_z == dest_z
                && (tile & collision::WALL_WEST) == collision::OPEN);
    }
    false
}

/// Interaction footprint of a loc. Width/height/access come from gamedata;
/// shape/angle from the world loc, tile default when there is no loc.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TargetDesc {
    pub width: i32,
    pub height: i32,
    pub angle: i32,
    pub shape: i32,
    pub block_access: u32,
}

pub fn describe_target(
    loc_shape_angle: Option<(i32, i32)>,
    width: i32,
    height: i32,
    block_access: u32,
) -> TargetDesc {
    match loc_shape_angle {
        None => TargetDesc {
            width: 1,
            height: 1,
            angle: 0,
            shape: SHAPE_TILE,
            block_access: 0,
        },
        Some((shape, angle)) => TargetDesc {
            width,
            height,
            angle,
            shape,
            block_access,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_flag(_x: i32, _z: i32, _level: i32) -> u32 {
        collision::OPEN
    }

    #[test]
    fn tile_reached_only_on_tile() {
        assert!(reached(&open_flag, 0, 5, 5, 5, 5, 1, 1, 0, SHAPE_TILE, 0));
        assert!(!reached(&open_flag, 0, 5, 6, 5, 5, 1, 1, 0, SHAPE_TILE, 0));
    }

    #[test]
    fn rect_reached_from_sides() {
        assert!(reached(&open_flag, 0, 4, 5, 5, 5, 1, 1, 0, 10, 0));
        assert!(reached(&open_flag, 0, 6, 5, 5, 5, 1, 1, 0, 10, 0));
        assert!(reached(&open_flag, 0, 5, 4, 5, 5, 1, 1, 0, 10, 0));
        assert!(reached(&open_flag, 0, 5, 6, 5, 5, 1, 1, 0, 10, 0));
        assert!(!reached(&open_flag, 0, 4, 4, 5, 5, 1, 1, 0, 10, 0));
    }

    #[test]
    fn straight_wall_reached_beside() {
        assert!(reached(&open_flag, 0, 4, 5, 5, 5, 1, 1, ANGLE_WEST, 0, 0));
        assert!(!reached(&open_flag, 0, 6, 5, 5, 5, 1, 1, ANGLE_WEST, 0, 0));
    }
}
