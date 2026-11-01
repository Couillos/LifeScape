"""Collision flags and map loading, mirroring the server routefinder.

Flags live on 8x8-tile zones keyed like the server. Map data comes from
the packed build output (data/pack/server/maps or maps-server.zip),
loc footprints from lifescape/data/gametypes.json.
"""

import json
import os
import zipfile

from .buffer import Reader

OPEN = 0x0
WALL_NORTH_WEST = 0x1
WALL_NORTH = 0x2
WALL_NORTH_EAST = 0x4
WALL_EAST = 0x8
WALL_SOUTH_EAST = 0x10
WALL_SOUTH = 0x20
WALL_SOUTH_WEST = 0x40
WALL_WEST = 0x80
LOC = 0x100
WALL_NORTH_WEST_PROJ = 0x200
WALL_NORTH_PROJ = 0x400
WALL_NORTH_EAST_PROJ = 0x800
WALL_EAST_PROJ = 0x1000
WALL_SOUTH_EAST_PROJ = 0x2000
WALL_SOUTH_PROJ = 0x4000
WALL_SOUTH_WEST_PROJ = 0x8000
WALL_WEST_PROJ = 0x10000
LOC_PROJ_BLOCKER = 0x20000
FLOOR_DECORATION = 0x40000
NPC_OCC = 0x80000
BLOCK_NPC_AND_PLAYERS = 0x100000
FLOOR = 0x200000
PLAYER_OCC = 0x400000
ROOF = 0x80000000
FLOOR_BLOCKED = 0x240000
WALK_BLOCKED = 0x240100
BLOCK_WEST = 0x240108
BLOCK_EAST = 0x240180
BLOCK_SOUTH = 0x240102
BLOCK_NORTH = 0x240120
BLOCK_SOUTH_WEST = 0x24010E
BLOCK_SOUTH_EAST = 0x240183
BLOCK_NORTH_WEST = 0x240138
BLOCK_NORTH_EAST = 0x2401E0


def tile_walkable(flags):
    """True unless every direction is blocked (roof/floor flags alone are fine)."""
    return not (
        (flags & BLOCK_NORTH)
        and (flags & BLOCK_EAST)
        and (flags & BLOCK_SOUTH)
        and (flags & BLOCK_WEST)
    )
NULL = 0x7FFFFFFF
MASK32 = 0xFFFFFFFF

WALL_STRAIGHT = 0
WALL_DIAGONAL_CORNER = 1
WALL_L = 2
WALL_SQUARE_CORNER = 3

ANGLE_WEST = 0
ANGLE_NORTH = 1
ANGLE_EAST = 2
ANGLE_SOUTH = 3

LAYER_WALL = 0
LAYER_WALL_DECOR = 1
LAYER_GROUND = 2
LAYER_GROUND_DECOR = 3

BLOCK_MAP_SQUARE = 0x1
LINK_BELOW = 0x2
REMOVE_ROOFS = 0x4

MAP_SIZE = 64
LEVELS = 4
LANDS = MAP_SIZE * MAP_SIZE * LEVELS


def zone_index(x, z, level):
    return ((x >> 3) & 0x7FF) | (((z >> 3) & 0x7FF) << 11) | ((level & 0x3) << 22)


def tile_index(x, z):
    return (x & 0x7) | ((z & 0x7) << 3)


def pack_land(x, z, level):
    return (z & 0x3F) | ((x & 0x3F) << 6) | ((level & 0x3) << 12)


class CollisionMap:
    def __init__(self):
        self.zones = {}

    def _zone(self, x, z, level, create=False):
        key = zone_index(x, z, level)
        zone = self.zones.get(key)
        if zone is None and create:
            zone = [0] * 64
            self.zones[key] = zone
        return zone

    def get(self, x, z, level):
        zone = self._zone(x, z, level)
        if zone is None:
            return NULL
        return zone[tile_index(x, z)]

    def has(self, x, z, level):
        """True when this tile belongs to a map zone we actually loaded."""
        return self._zone(x, z, level) is not None

    def copy(self):
        other = CollisionMap()
        other.zones = {key: list(zone) for key, zone in self.zones.items()}
        return other

    def is_flagged(self, x, z, level, mask):
        return (self.get(x, z, level) & mask) != OPEN

    def set(self, x, z, level, mask):
        zone = self._zone(x, z, level, create=True)
        zone[tile_index(x, z)] = mask & MASK32

    def add(self, x, z, level, mask):
        zone = self._zone(x, z, level, create=True)
        slot = tile_index(x, z)
        zone[slot] = (zone[slot] | mask) & MASK32

    def remove(self, x, z, level, mask):
        zone = self._zone(x, z, level, create=True)
        slot = tile_index(x, z)
        zone[slot] = (zone[slot] & ~mask) & MASK32

    def change_floor(self, x, z, level, add):
        if add:
            self.add(x, z, level, FLOOR)
        else:
            self.remove(x, z, level, FLOOR)

    def change_roof(self, x, z, level, add):
        if add:
            self.add(x, z, level, ROOF)
        else:
            self.remove(x, z, level, ROOF)

    def change_npc(self, x, z, level, size, add):
        self._square(x, z, level, size, NPC_OCC, add)

    def change_block(self, x, z, level, size, add):
        self._square(x, z, level, size, BLOCK_NPC_AND_PLAYERS, add)

    def change_player_occ(self, x, z, level, size, add):
        self._square(x, z, level, size, PLAYER_OCC, add)

    def _square(self, x, z, level, size, mask, add):
        for index in range(size * size):
            dx = x + index % size
            dz = z + index // size
            if add:
                self.add(dx, dz, level, mask)
            else:
                self.remove(dx, dz, level, mask)

    def change_loc(self, x, z, level, width, length, blockrange, add):
        mask = LOC
        if blockrange:
            mask |= LOC_PROJ_BLOCKER
        for index in range(width * length):
            dx = x + index % width
            dz = z + index // width
            if add:
                self.add(dx, dz, level, mask)
            else:
                self.remove(dx, dz, level, mask)

    def change_wall(self, x, z, level, angle, shape, blockrange, add):
        if shape == WALL_STRAIGHT:
            self._wall_straight(x, z, level, angle, blockrange, add)
        elif shape in (WALL_DIAGONAL_CORNER, WALL_SQUARE_CORNER):
            self._wall_corner(x, z, level, angle, blockrange, add)
        elif shape == WALL_L:
            self._wall_l(x, z, level, angle, blockrange, add)

    def _wall_mask(self, normal, projectile, blockrange):
        return projectile if blockrange else normal

    def _wall_straight(self, x, z, level, angle, blockrange, add):
        west = self._wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange)
        east = self._wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange)
        north = self._wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange)
        south = self._wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange)
        if angle == ANGLE_WEST:
            self._wall_pair(x, z, level, west, x - 1, z, level, east, add)
        elif angle == ANGLE_NORTH:
            self._wall_pair(x, z, level, north, x, z + 1, level, south, add)
        elif angle == ANGLE_EAST:
            self._wall_pair(x, z, level, east, x + 1, z, level, west, add)
        elif angle == ANGLE_SOUTH:
            self._wall_pair(x, z, level, south, x, z - 1, level, north, add)
        if blockrange:
            self._wall_straight(x, z, level, angle, False, add)

    def _wall_corner(self, x, z, level, angle, blockrange, add):
        north_west = self._wall_mask(WALL_NORTH_WEST, WALL_NORTH_WEST_PROJ, blockrange)
        south_east = self._wall_mask(WALL_SOUTH_EAST, WALL_SOUTH_EAST_PROJ, blockrange)
        north_east = self._wall_mask(WALL_NORTH_EAST, WALL_NORTH_EAST_PROJ, blockrange)
        south_west = self._wall_mask(WALL_SOUTH_WEST, WALL_SOUTH_WEST_PROJ, blockrange)
        if angle == ANGLE_WEST:
            self._wall_pair(x, z, level, north_west, x - 1, z + 1, level, south_east, add)
        elif angle == ANGLE_NORTH:
            self._wall_pair(x, z, level, north_east, x + 1, z + 1, level, south_west, add)
        elif angle == ANGLE_EAST:
            self._wall_pair(x, z, level, south_east, x + 1, z - 1, level, north_west, add)
        elif angle == ANGLE_SOUTH:
            self._wall_pair(x, z, level, south_west, x - 1, z - 1, level, north_east, add)
        if blockrange:
            self._wall_corner(x, z, level, angle, False, add)

    def _wall_l(self, x, z, level, angle, blockrange, add):
        west = self._wall_mask(WALL_WEST, WALL_WEST_PROJ, blockrange)
        east = self._wall_mask(WALL_EAST, WALL_EAST_PROJ, blockrange)
        north = self._wall_mask(WALL_NORTH, WALL_NORTH_PROJ, blockrange)
        south = self._wall_mask(WALL_SOUTH, WALL_SOUTH_PROJ, blockrange)
        if angle == ANGLE_WEST:
            self._wall_single(x, z, level, north | west, add)
            self._wall_single(x - 1, z, level, east, add)
            self._wall_single(x, z + 1, level, south, add)
        elif angle == ANGLE_NORTH:
            self._wall_single(x, z, level, north | east, add)
            self._wall_single(x + 1, z, level, west, add)
            self._wall_single(x, z + 1, level, south, add)
        elif angle == ANGLE_EAST:
            self._wall_single(x, z, level, south | east, add)
            self._wall_single(x + 1, z, level, west, add)
            self._wall_single(x, z - 1, level, north, add)
        elif angle == ANGLE_SOUTH:
            self._wall_single(x, z, level, south | west, add)
            self._wall_single(x, z - 1, level, north, add)
            self._wall_single(x - 1, z, level, east, add)
        if blockrange:
            self._wall_l(x, z, level, angle, False, add)

    def _wall_pair(self, x1, z1, y1, mask1, x2, z2, y2, mask2, add):
        self._wall_single(x1, z1, y1, mask1, add)
        self._wall_single(x2, z2, y2, mask2, add)

    def _wall_single(self, x, z, level, mask, add):
        if add:
            self.add(x, z, level, mask)
        else:
            self.remove(x, z, level, mask)


def layer_of(shape):
    if shape in (0, 1, 2, 3):
        return LAYER_WALL
    if shape in (4, 5, 6, 7, 8):
        return LAYER_WALL_DECOR
    if shape == 22:
        return LAYER_GROUND_DECOR
    return LAYER_GROUND


def change_loc_collision(collision, shape, angle, blockrange, length, width, active, x, z, level, add):
    layer = layer_of(shape)
    if layer == LAYER_WALL:
        collision.change_wall(x, z, level, angle, shape, blockrange, add)
    elif layer == LAYER_GROUND:
        if angle in (ANGLE_NORTH, ANGLE_SOUTH):
            collision.change_loc(x, z, level, length, width, blockrange, add)
        else:
            collision.change_loc(x, z, level, width, length, blockrange, add)
    elif layer == LAYER_GROUND_DECOR:
        if active == 1:
            collision.change_floor(x, z, level, add)


def load_types(path):
    with open(path) as handle:
        return json.load(handle)


def load_maps(pack_dir, loctypes, members=True):
    collision = CollisionMap()
    static_locs = {}
    maps = _map_files(pack_dir)
    for square_x, square_z, ground, locs in maps:
        _load_ground(collision, ground, square_x, square_z, members)
        lands = _read_lands(ground)
        _load_locs(collision, lands, locs, square_x, square_z, loctypes, members, static_locs)
    return collision, static_locs


def _map_files(pack_dir):
    zipped = os.path.join(pack_dir, ".cache", "maps-server.zip")
    if os.path.exists(zipped):
        with zipfile.ZipFile(zipped) as archive:
            names = archive.namelist()
            squares = sorted({name[1:].split("_")[0] + "_" + name[1:].split("_")[1] for name in names if name.startswith("m")})
            for square in squares:
                mx, mz = (int(part) for part in square.split("_"))
                yield mx << 6, mz << 6, archive.read("m%s" % square), archive.read("l%s" % square)
        return
    maps_dir = os.path.join(pack_dir, "server", "maps")
    for name in sorted(os.listdir(maps_dir)):
        if not name.startswith("m"):
            continue
        mx, mz = (int(part) for part in name[1:].split("_"))
        with open(os.path.join(maps_dir, "m%s_%s" % (mx, mz)), "rb") as handle:
            ground = handle.read()
        with open(os.path.join(maps_dir, "l%s_%s" % (mx, mz)), "rb") as handle:
            locs = handle.read()
        yield mx << 6, mz << 6, ground, locs


def _read_lands(ground):
    lands = [0] * LANDS
    packet = Reader(ground)
    for level in range(LEVELS):
        for x in range(MAP_SIZE):
            for z in range(MAP_SIZE):
                while True:
                    opcode = packet.u8()
                    if opcode == 0:
                        break
                    if opcode == 1:
                        packet.pos += 1
                        break
                    if opcode <= 49:
                        packet.pos += 1
                    elif opcode <= 81:
                        lands[pack_land(x, z, level)] = opcode - 49
    return lands


def _load_ground(collision, ground, square_x, square_z, members):
    lands = _read_lands(ground)
    for level in range(LEVELS):
        for x in range(MAP_SIZE):
            absolute_x = x + square_x
            for z in range(MAP_SIZE):
                absolute_z = z + square_z
                if x % 7 == 0 and z % 7 == 0:
                    collision._zone(absolute_x, absolute_z, level, create=True)
                land = lands[pack_land(x, z, level)]
                if land & REMOVE_ROOFS:
                    collision.change_roof(absolute_x, absolute_z, level, True)
                if not land & BLOCK_MAP_SQUARE:
                    continue
                if level == 1:
                    bridged = lands[pack_land(x, z, level)] & LINK_BELOW
                else:
                    bridged = lands[pack_land(x, z, 1)] & LINK_BELOW
                actual = level - 1 if bridged else level
                if actual < 0:
                    continue
                collision.change_floor(absolute_x, absolute_z, actual, True)


def _load_locs(collision, lands, locs, square_x, square_z, loctypes, members, static_locs):
    packet = Reader(locs)
    loc_id = -1
    offset = packet.smarts()
    while offset != 0:
        loc_id += offset
        coord = 0
        coord_offset = packet.smarts()
        while coord_offset != 0:
            coord += coord_offset - 1
            z = coord & 0x3F
            x = (coord >> 6) & 0x3F
            level = (coord >> 12) & 0x3
            info = packet.u8()
            coord_offset = packet.smarts()
            absolute_x = x + square_x
            absolute_z = z + square_z
            if level == 1:
                bridged = lands[coord] & LINK_BELOW
            else:
                bridged = lands[pack_land(x, z, 1)] & LINK_BELOW
            actual = level - 1 if bridged else level
            if actual < 0:
                continue
            shape = info >> 2
            angle = info & 0x3
            static_locs[(absolute_x, absolute_z, actual)] = (loc_id, shape, angle)
            loc = loctypes["loc"][loc_id] if loc_id < len(loctypes["loc"]) else None
            if loc is None or not loc["blockwalk"]:
                continue
            change_loc_collision(
                collision, shape, angle, loc["blockrange"], loc["length"], loc["width"], loc["active"], absolute_x, absolute_z, actual, True
            )
        offset = packet.smarts()
