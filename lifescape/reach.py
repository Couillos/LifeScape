"""Reach checks: quand le bot est "assez pres" sans marcher dessus.

Port de ReachStrategy du serveur (murs, decors, rectangles). Taille 1
uniquement : le bot est toujours un joueur solo.
"""

from . import collision as flags

# Valeurs loc : memes que le serveur (flags.ts).
SHAPE_TILE = -1
SHAPE_NPC = -2

ANGLE_WEST = 0
ANGLE_NORTH = 1
ANGLE_EAST = 2
ANGLE_SOUTH = 3

BLOCK_NORTH = 0x1
BLOCK_EAST = 0x2
BLOCK_SOUTH = 0x4
BLOCK_WEST = 0x8


def rotate(angle, a, b):
    return b if angle & 0x1 else a


def rotate_flags(angle, block_access):
    if angle == 0:
        return block_access
    return (((block_access << angle) & 0xF) | (block_access >> (4 - angle))) & 0xF


def collides(src_x, src_z, dest_x, dest_z, src_w, src_h, dest_w, dest_h):
    if src_x >= dest_x + dest_w or src_x + src_w <= dest_x:
        return False
    return src_z < dest_z + dest_h and dest_z < src_h + src_z


def _exit_strategy(shape):
    if shape == SHAPE_NPC:
        return "exclusive"
    if shape == SHAPE_TILE:
        return "none"
    if (0 <= shape <= 3) or shape == 9:
        return "wall"
    if shape < 9:
        return "decor"
    if shape in (10, 11) or shape == 22:
        return "rect"
    return "none"


def altered_rotation(angle, shape):
    return (angle + 2) & 0x3 if shape == 7 else angle


def reached(get_flag, level, src_x, src_z, dest_x, dest_z,
           dest_width=1, dest_height=1, angle=0, shape=SHAPE_TILE, block_access=0):
    """True quand (src_x, src_z) suffit pour interagir avec la cible."""
    strategy = _exit_strategy(shape)
    if strategy != "exclusive" and src_x == dest_x and src_z == dest_z:
        return True
    if strategy == "wall":
        return _reach_wall(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle)
    if strategy == "decor":
        return _reach_decor(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle)
    if strategy == "rect":
        return _reach_rect(get_flag, level, src_x, src_z, dest_x, dest_z,
                           dest_width, dest_height, angle, block_access)
    if strategy == "exclusive":
        return (not collides(src_x, src_z, dest_x, dest_z, 1, 1, dest_width, dest_height)
                and _reach_rect(get_flag, level, src_x, src_z, dest_x, dest_z,
                                dest_width, dest_height, angle, block_access))
    return False


def _reach_rect(get_flag, level, src_x, src_z, dest_x, dest_z,
                dest_width, dest_height, angle, block_access):
    width = rotate(angle, dest_width, dest_height)
    height = rotate(angle, dest_height, dest_width)
    access = rotate_flags(angle, block_access)
    if collides(src_x, src_z, dest_x, dest_z, 1, 1, width, height):
        return True
    return _reach_rect1(get_flag, level, src_x, src_z, dest_x, dest_z, width, height, access)


def _reach_rect1(get_flag, level, src_x, src_z, dest_x, dest_z, width, height, access):
    east = dest_x + width - 1
    north = dest_z + height - 1
    if (src_x == dest_x - 1 and dest_z <= src_z <= north
            and not get_flag(src_x, src_z, level) & flags.WALL_EAST
            and not access & BLOCK_WEST):
        return True
    if (src_x == east + 1 and dest_z <= src_z <= north
            and not get_flag(src_x, src_z, level) & flags.WALL_WEST
            and not access & BLOCK_EAST):
        return True
    if (src_z + 1 == dest_z and dest_x <= src_x <= east
            and not get_flag(src_x, src_z, level) & flags.WALL_NORTH
            and not access & BLOCK_SOUTH):
        return True
    return (src_z == north + 1 and dest_x <= src_x <= east
            and not get_flag(src_x, src_z, level) & flags.WALL_SOUTH
            and not access & BLOCK_NORTH)


def _reach_wall(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle):
    if src_x == dest_x and src_z == dest_z:
        return True
    tile = get_flag(src_x, src_z, level)
    if shape == 0:
        if angle == ANGLE_WEST:
            return ((src_x == dest_x - 1 and src_z == dest_z)
                    or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.BLOCK_NORTH)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.BLOCK_SOUTH))
        if angle == ANGLE_NORTH:
            return ((src_x == dest_x and src_z == dest_z + 1)
                    or (src_x == dest_x - 1 and src_z == dest_z and not tile & flags.BLOCK_WEST)
                    or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.BLOCK_EAST))
        if angle == ANGLE_EAST:
            return ((src_x == dest_x + 1 and src_z == dest_z)
                    or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.BLOCK_NORTH)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.BLOCK_SOUTH))
        return ((src_x == dest_x and src_z == dest_z - 1)
                or (src_x == dest_x - 1 and src_z == dest_z and not tile & flags.BLOCK_WEST)
                or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.BLOCK_EAST))
    if shape == 2:
        if angle == ANGLE_WEST:
            return ((src_x == dest_x - 1 and src_z == dest_z)
                    or (src_x == dest_x and src_z == dest_z + 1)
                    or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.BLOCK_EAST)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.BLOCK_SOUTH))
        if angle == ANGLE_NORTH:
            return ((src_x == dest_x - 1 and src_z == dest_z and not tile & flags.BLOCK_WEST)
                    or (src_x == dest_x and src_z == dest_z + 1)
                    or (src_x == dest_x + 1 and src_z == dest_z)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.BLOCK_SOUTH))
        if angle == ANGLE_EAST:
            return ((src_x == dest_x - 1 and src_z == dest_z and not tile & flags.BLOCK_WEST)
                    or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.BLOCK_NORTH)
                    or (src_x == dest_x + 1 and src_z == dest_z)
                    or (src_x == dest_x and src_z == dest_z - 1))
        return ((src_x == dest_x - 1 and src_z == dest_z)
                or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.BLOCK_NORTH)
                or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.BLOCK_EAST)
                or (src_x == dest_x and src_z == dest_z - 1))
    if shape == 1:
        return ((src_x == dest_x and src_z == dest_z + 1 and not tile & flags.WALL_SOUTH)
                or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.WALL_NORTH)
                or (src_x == dest_x - 1 and src_z == dest_z and not tile & flags.WALL_EAST)
                or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.WALL_WEST))
    return False


def _reach_decor(get_flag, level, src_x, src_z, dest_x, dest_z, shape, angle):
    if src_x == dest_x and src_z == dest_z:
        return True
    tile = get_flag(src_x, src_z, level)
    if shape in (6, 7):
        rotation = altered_rotation(angle, shape)
        if rotation == ANGLE_WEST:
            return ((src_x == dest_x + 1 and src_z == dest_z and not tile & flags.WALL_WEST)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.WALL_NORTH))
        if rotation == ANGLE_NORTH:
            return ((src_x == dest_x - 1 and src_z == dest_z and not tile & flags.WALL_EAST)
                    or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.WALL_NORTH))
        if rotation == ANGLE_EAST:
            return ((src_x == dest_x - 1 and src_z == dest_z and not tile & flags.WALL_EAST)
                    or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.WALL_SOUTH))
        return ((src_x == dest_x + 1 and src_z == dest_z and not tile & flags.WALL_WEST)
                or (src_x == dest_x and src_z == dest_z + 1 and not tile & flags.WALL_SOUTH))
    if shape == 8:
        return ((src_x == dest_x and src_z == dest_z + 1 and not tile & flags.WALL_SOUTH)
                or (src_x == dest_x and src_z == dest_z - 1 and not tile & flags.WALL_NORTH)
                or (src_x == dest_x - 1 and src_z == dest_z and not tile & flags.WALL_EAST)
                or (src_x == dest_x + 1 and src_z == dest_z and not tile & flags.WALL_WEST))
    return False


def describe_target(world_loc, loctypes, x, z, level):
    """(width, height, angle, shape, block_access) d'un loc, ou defaut tuile.

    shortcut: murs et decors speciaux (toits 12-21) retombent sur "tuile
    exacte" ; seuls les cas utiles au bot (murs, objets, decors) sont
    detailles ci-dessus.
    """
    if world_loc is None:
        return (1, 1, 0, SHAPE_TILE, 0)
    try:
        loc_id, shape, angle = world_loc[0], world_loc[1], world_loc[2]
    except (IndexError, TypeError, ValueError):
        return (1, 1, 0, SHAPE_TILE, 0)
    width, height, access = 1, 1, 0
    try:
        entry = loctypes["loc"][loc_id] if loctypes is not None else None
    except (IndexError, TypeError, KeyError):
        entry = None
    if entry:
        try:
            width = int(entry.get("width", 1) or 1)
            height = int(entry.get("length", 1) or 1)
        except (TypeError, ValueError):
            pass
        try:
            access = int(entry.get("blockaccess", 0) or 0)
        except (TypeError, ValueError):
            access = 0
    return (width, height, angle, shape, access)
