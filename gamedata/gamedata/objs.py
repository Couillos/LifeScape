"""Game objects: ids, names, texts, actions, shop prices, alchemy values."""

from dataclasses import dataclass, field

from gamedata import base


@dataclass(frozen=True)
class Obj:
    obj_id: int
    name: str | None
    display: str | None = None
    desc: str | None = None
    ops: tuple = ()
    inv_ops: tuple = ()
    price: int | None = None
    alch_high: int | None = None
    alch_low: int | None = None
    stackable: bool = False
    members: bool = False
    respawn_ticks: int = 100


def _from_row(obj_id, row):
    if row is None:
        return Obj(obj_id=obj_id, name=None)
    return Obj(
        obj_id=obj_id,
        name=row.get("name"),
        display=row.get("display") or row.get("name"),
        desc=row.get("desc"),
        ops=tuple(row.get("ops") or ()),
        inv_ops=tuple(row.get("iops") or ()),
        price=row.get("price", 0),
        alch_high=row.get("alch_high", 0),
        alch_low=row.get("alch_low", 0),
        stackable=bool(row.get("stackable", False)),
        members=bool(row.get("members", False)),
        respawn_ticks=row.get("respawn_ticks", 100),
    )


_alls = None
_names = None


def all_objs():
    global _alls
    if _alls is None:
        table = base.load_table("objs")
        _alls = [_from_row(obj_id, row) for obj_id, row in enumerate(table)]
    return _alls


def by_id(obj_id):
    entries = all_objs()
    if obj_id < 0 or obj_id >= len(entries):
        return None
    return entries[obj_id]


def display_name_by_id(obj_id):
    """Name as the game shows it ("Air rune"), not the debugname."""
    entries = all_objs()
    if obj_id < 0 or obj_id >= len(entries):
        return None
    entry = entries[obj_id]
    return entry.display or entry.name


def name_by_id(obj_id):
    """Just the debugname: a flat id->name index, built once (UI hot path)."""
    global _names
    if _names is None:
        _names = {
            index: (row.get("name") if row else None)
            for index, row in enumerate(base.load_table("objs"))
        }
    return _names.get(obj_id)


def by_name(name):
    wanted = name.lower()
    return [entry for entry in all_objs() if (entry.name or "").lower() == wanted]


def search(fragment):
    wanted = fragment.lower()
    return [entry for entry in all_objs() if wanted in (entry.name or "").lower()]
