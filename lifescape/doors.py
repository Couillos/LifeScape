"""Openable walls: any loc with an Open option and a wall shape.

Closed wall doors block routing until clicked. The bot assumes they open,
walks up to them, clicks, and re-paths once the zone update lands.
Doors that refuse to open go on a per-bot blocklist.
"""

from . import collision as collision_mod

WALL_SHAPES = {0, 1, 2, 3}


DOOR_NAMES = ("door", "gate", "trap", "manhole", "portcullis", "fence")
CONTAINERS = ("chest", "crate", "cupboard", "drawer", "wardrobe", "bookcase", "cabinet", "barrel", "sack")


def open_op(loctypes, loc_id):
    for index, name in enumerate(loctypes["loc"][loc_id]["ops"]):
        if name and name.lower() == "open":
            return index + 1
    return None


def is_door(loctypes, loc_id):
    """Any Open loc is a possible route blocker; containers are not.

    Many gates keep a generic ``loc_<id>`` debugname (e.g. the Lumbridge
    cow-field gate), so the name alone cannot decide.
    """
    if not 0 <= loc_id < len(loctypes["loc"]):
        return False
    name = (loctypes["loc"][loc_id].get("name") or "").lower()
    if any(key in name for key in CONTAINERS):
        return False
    if any(key in name for key in ("trapdoor", "ladder", "stair", "portal", "manhole")):
        return False
    return True


def find_openables(pack_dir, loctypes):
    placements = []
    for square_x, square_z, ground, locs in collision_mod._map_files(pack_dir):
        lands = collision_mod._read_lands(ground)
        packet = collision_mod.Reader(locs)
        loc_id = -1
        offset = packet.smarts()
        while offset != 0:
            loc_id += offset
            coord = 0
            coord_offset = packet.smarts()
            while coord_offset != 0:
                coord += coord_offset - 1
                x = (coord >> 6) & 0x3F
                z = coord & 0x3F
                level = (coord >> 12) & 0x3
                info = packet.u8()
                coord_offset = packet.smarts()
                loc = loctypes["loc"][loc_id] if 0 <= loc_id < len(loctypes["loc"]) else None
                if loc is None or open_op(loctypes, loc_id) is None:
                    continue
                shape = info >> 2
                angle = info & 0x3
                if level == 1:
                    bridged = lands[coord] & collision_mod.LINK_BELOW
                else:
                    bridged = lands[collision_mod.pack_land(x, z, 1)] & collision_mod.LINK_BELOW
                actual = level - 1 if bridged else level
                if actual < 0:
                    continue
                placements.append((square_x + x, square_z + z, actual, shape, angle, loc["blockrange"], loc["length"], loc["width"], loc["active"], loc_id))
            offset = packet.smarts()
    return placements


def footprint(placement):
    x, z, level, shape, angle, blockrange, length, width, active, _loc = placement
    # A wall-shaped loc (doors, gates) occupies exactly its own tile. The
    # collision engine also writes a wall flag on the neighbour tile, but that
    # tile is not part of the doorway: routing "through the door" onto it
    # walks the player into a closed-door pocket.
    if shape <= 3:
        return {(x, z, level)}
    probe = collision_mod.CollisionMap()
    collision_mod.change_loc_collision(probe, shape, angle, blockrange, length, width, active, x, z, level, True)
    tiles = set()
    for key, zone in probe.zones.items():
        base_x = (key & 0x7FF) << 3
        base_z = ((key >> 11) & 0x7FF) << 3
        level = (key >> 22) & 0x3
        for index, flags in enumerate(zone):
            # Movement blockers only: the collision engine also writes
            # projectile-blocker flags on the tiles beside a wall, which used
            # to grow a 1-tile gate into a 2-tile footprint and made the bot
            # think it was standing on the door.
            blocked = flags & (
                collision_mod.WALK_BLOCKED
                | collision_mod.WALL_NORTH
                | collision_mod.WALL_EAST
                | collision_mod.WALL_SOUTH
                | collision_mod.WALL_WEST
            )
            if blocked:
                tiles.add((base_x + (index & 0x7), base_z + ((index >> 3) & 0x7), level))
    # Some gates carry no collision flags of their own (the map already has
    # the opening): they still occupy their own tile and must stay visible to
    # the "door on the path" scan, otherwise the bot walks into a closed gate.
    if not tiles:
        tiles.add((x, z, level))
    return tiles
