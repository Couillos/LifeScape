"""Door-aware navigation planning over the cached collision map.

The planner treats every openable door footprint as walkable while
planning, then annotates the first tile of each door crossed. The walker
walks up to a door, opens it, and continues with the same plan. That
keeps the whole route visible (and the next action known) even before
the door is open.
"""

from dataclasses import dataclass

from . import collision as collision_mod
from . import f2p as f2p_mod
from . import steps as steps_mod


@dataclass(frozen=True)
class Action:
    kind: str
    x: int
    z: int
    level: int
    loc_id: int
    name: str
    op: int


@dataclass(frozen=True)
class Plan:
    steps: tuple
    actions: tuple
    target: tuple


class DoorMap:
    """Collision view where a set of door tiles is open for planning."""

    def __init__(self, base, door_tiles, freemap=None):
        self.base = base
        self.door_tiles = door_tiles
        self.freemap = freemap

    def get(self, x, z, level):
        if self.freemap is not None and not f2p_mod.is_f2p(self.freemap, x, z):
            return collision_mod.WALK_BLOCKED
        if (x, z, level) in self.door_tiles:
            return collision_mod.OPEN
        return self.base.get(x, z, level)


class Planner:
    def __init__(self, router, collision, loc_types, wall_doors, freemap=None):
        self.router = router
        self.collision = collision
        self.loc_types = loc_types
        self.freemap = freemap
        self.wall_doors = tuple(wall_doors)
        self.door_tiles = {}
        self.single_tiles = {}
        self.footprints = {}
        for spot in self.wall_doors:
            tiles = {(x, z, level) for x, z, level in _footprint(spot)}
            self.footprints[id(spot)] = tiles
            category = loc_types["loc"][spot[9]].get("category", -1) if spot[9] < len(loc_types["loc"]) else -1
            for tile in tiles:
                self.door_tiles[tile] = spot
                if category not in (21, 23):
                    self.single_tiles[tile] = spot

    def closed_door_at(self, x, z, level, is_closed):
        spot = self.door_tiles.get((x, z, level))
        if spot is None:
            return None
        return spot if is_closed(spot) else None

    def plan(self, level, start_x, start_z, dest_x, dest_z, is_closed, blocked_doors=(), base=None):
        blocked_tiles = set()
        for spot in self.wall_doors:
            if id(spot) in blocked_doors:
                blocked_tiles |= self.footprints[id(spot)]
        # Prefer ordinary doors: double doors swing onto adjacent tiles and
        # their open halves can block the doorway on the client map.
        base = self.collision if base is None else base
        path = self.router.find_path(
            DoorMap(base, set(self.single_tiles) - blocked_tiles, self.freemap),
            level, start_x, start_z, dest_x, dest_z,
        )
        if not path:
            path = self.router.find_path(
                DoorMap(base, set(self.door_tiles) - blocked_tiles, self.freemap),
                level, start_x, start_z, dest_x, dest_z,
            )
        if not path:
            return None
        steps = tuple(steps_mod.expand_waypoints(path, (start_x, start_z)))
        actions = []
        seen = set()
        for index, (step_x, step_z) in enumerate(steps):
            spot = self.door_tiles.get((step_x, step_z, level))
            if spot is None or id(spot) in blocked_doors or not is_closed(spot):
                continue
            if id(spot) in seen:
                continue
            seen.add(id(spot))
            actions.append((index, Action(
                kind="door",
                x=spot[0],
                z=spot[1],
                level=spot[2],
                loc_id=spot[9],
                name=(self.loc_types["loc"][spot[9]].get("name") or "") if spot[9] < len(self.loc_types["loc"]) else "",
                op=_open_op(self.loc_types, spot[9]),
            )))
        return Plan(steps=steps, actions=tuple(actions), target=(dest_x, dest_z))


def _open_op(loc_types, loc_id):
    for index, name in enumerate(loc_types["loc"][loc_id]["ops"]):
        if name and name.lower() == "open":
            return index + 1
    return None


def _footprint(spot):
    x, z, level, shape, angle, blockrange, length, width, active, _loc = spot
    if shape <= 3:
        return [(x, z, level)]
    probe = collision_mod.CollisionMap()
    collision_mod.change_loc_collision(probe, shape, angle, blockrange, length, width, active, x, z, level, True)
    tiles = []
    for key, zone in probe.zones.items():
        base_x = (key & 0x7FF) << 3
        base_z = ((key >> 11) & 0x7FF) << 3
        tile_level = (key >> 22) & 0x3
        for index, flags in enumerate(zone):
            if flags:
                tiles.append((base_x + (index & 0x7), base_z + ((index >> 3) & 0x7), tile_level))
    return tiles
