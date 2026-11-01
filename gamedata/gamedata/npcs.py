"""Non-player characters: ids, names, sizes, interactions."""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class SpawnPoint:
    x: int
    z: int
    level: int


@dataclass(frozen=True)
class Npc:
    npc_id: int
    name: str | None
    size: int = 1
    ops: tuple = ()
    members: bool = False
    huntrange: int = 0
    vislevel: int = -1
    hunts_players: bool = False
    spawns: tuple = ()


def _from_row(npc_id, row):
    if row is None:
        return Npc(npc_id=npc_id, name=None)
    return Npc(
        npc_id=npc_id,
        name=row.get("name"),
        size=row.get("size", 1),
        ops=tuple(row.get("ops") or ()),
        members=bool(row.get("members", False)),
        huntrange=row.get("huntrange", 0) or 0,
        vislevel=row.get("vislevel", -1),
        hunts_players=bool(row.get("hunts_players", False)),
        spawns=tuple(SpawnPoint(x=spot["x"], z=spot["z"], level=spot["level"]) for spot in row.get("spawns", []) if isinstance(spot, dict)),
    )


_alls = None
_names = None


def all_npcs():
    global _alls
    if _alls is None:
        table = base.load_table("npcs")
        _alls = [_from_row(npc_id, row) for npc_id, row in enumerate(table)]
    return _alls


def by_id(npc_id):
    entries = all_npcs()
    if npc_id < 0 or npc_id >= len(entries):
        return None
    return entries[npc_id]


def name_by_id(npc_id):
    """Just the name: a flat id->name index, built once (UI hot path)."""
    global _names
    if _names is None:
        _names = {
            index: (row.get("name") if row else None)
            for index, row in enumerate(base.load_table("npcs"))
        }
    return _names.get(npc_id)


def by_name(name):
    wanted = name.lower()
    return [entry for entry in all_npcs() if (entry.name or "").lower() == wanted]


def search(fragment):
    wanted = fragment.lower()
    return [entry for entry in all_npcs() if wanted in (entry.name or "").lower()]
