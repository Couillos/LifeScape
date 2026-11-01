"""Scenery (locs): ids, names, footprints, collision, interactions."""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class SpawnLoc:
    x: int
    z: int
    level: int
    members: bool | None = None


@dataclass(frozen=True)
class Loc:
    loc_id: int
    name: str | None
    width: int = 1
    length: int = 1
    blockwalk: bool = False
    blockrange: bool = False
    active: int = 0
    ops: tuple = ()
    spawns: tuple = ()


def _from_row(loc_id, row):
    if row is None:
        return Loc(loc_id=loc_id, name=None)
    return Loc(
        loc_id=loc_id,
        name=row.get("name"),
        width=row.get("width", 1),
        length=row.get("length", 1),
        blockwalk=bool(row.get("blockwalk", False)),
        blockrange=bool(row.get("blockrange", False)),
        active=row.get("active", 0),
        ops=tuple(row.get("ops") or ()),
        spawns=tuple(
            SpawnLoc(x=spot["x"], z=spot["z"], level=spot["level"], members=spot.get("members"))
            for spot in row.get("spawns", [])
            if isinstance(spot, dict)
        ),
    )


_alls = None
_names = None


def all_locs():
    global _alls
    if _alls is None:
        table = base.load_table("locs")
        _alls = [_from_row(loc_id, row) for loc_id, row in enumerate(table)]
    return _alls


def by_id(loc_id):
    entries = all_locs()
    if loc_id < 0 or loc_id >= len(entries):
        return None
    return entries[loc_id]


def name_by_id(loc_id):
    """Just the name: a flat id->name index, built once (UI hot path)."""
    global _names
    if _names is None:
        _names = {
            index: (row.get("name") if row else None)
            for index, row in enumerate(base.load_table("locs"))
        }
    return _names.get(loc_id)


def by_name(name):
    wanted = name.lower()
    return [entry for entry in all_locs() if (entry.name or "").lower() == wanted]


def search(fragment):
    wanted = fragment.lower()
    return [entry for entry in all_locs() if wanted in (entry.name or "").lower()]


def loc_spawns_near(names, x, z, level, radius=15):
    if isinstance(names, str):
        names = (names,)
    wanted = set(name.lower() for name in names)
    found = []
    for entry in all_locs():
        if (entry.name or "").lower() not in wanted:
            continue
        for spot in entry.spawns:
            if spot.level != level:
                continue
            if abs(spot.x - x) + abs(spot.z - z) <= radius:
                found.append(spot)
    return found
