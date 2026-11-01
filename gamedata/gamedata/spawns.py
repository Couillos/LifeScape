"""Static spawns: NPC positions and ground-item pops with cooldowns.

NPC spawns live inside npcs.json entries (see Npc.spawns); ground spawns
carry the respawn cooldown of their object type (ObjType.respawnrate,
ticks at 0.6 s).
"""

from dataclasses import dataclass

from gamedata import base
from gamedata import objs as objs_module


@dataclass(frozen=True)
class NpcSpawn:
    x: int
    z: int
    level: int
    npc_id: int
    members: bool = False


@dataclass(frozen=True)
class ObjSpawn:
    x: int
    z: int
    level: int
    obj_id: int
    count: int
    respawn_ticks: int
    members: bool = False


def all_npc_spawns(npc_id=None):
    table = base.load_table("npcs")
    out = []
    for row_id, row in enumerate(table):
        if row is None:
            continue
        if npc_id is not None and row_id != npc_id:
            continue
        for spot in row.get("spawns", []):
            out.append(NpcSpawn(x=spot["x"], z=spot["z"], level=spot["level"], npc_id=row_id, members=bool(spot.get("members", False))))
    return out


def all_obj_spawns(obj_id=None):
    table = base.load_table("spawns_obj")
    out = []
    for row in table:
        if obj_id is not None and row.get("obj") != obj_id:
            continue
        entry = objs_module.by_id(row["obj"])
        ticks = entry.respawn_ticks if entry is not None else 100
        row_members = row.get("members")
        if row_members is None:
            entry = objs_module.by_id(row["obj"])
            row_members = entry.members if entry is not None else False
        out.append(ObjSpawn(x=row["x"], z=row["z"], level=row["level"], obj_id=row["obj"], count=row.get("count", 1), respawn_ticks=ticks, members=bool(row_members)))
    return out


def obj_spawns_near(x, z, level, radius=15):
    found = []
    for spawn in all_obj_spawns():
        if spawn.level != level:
            continue
        if abs(spawn.x - x) + abs(spawn.z - z) <= radius:
            found.append(spawn)
    return found


def npc_spawns_near(x, z, level, radius=15):
    found = []
    for spawn in all_npc_spawns():
        if spawn.level != level:
            continue
        if abs(spawn.x - x) + abs(spawn.z - z) <= radius:
            found.append(spawn)
    return found
