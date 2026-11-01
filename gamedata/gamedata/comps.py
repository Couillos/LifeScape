"""Interface components: com ids by dotted name (magic:wind_strike)."""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class Comp:
    com_id: int
    name: str


def all_comps():
    table = base.load_table("comps")
    return [Comp(com_id=row["id"], name=row["name"]) for row in table if row]


_by_id_cache = None
_by_name_cache = None


def _index():
    global _by_id_cache, _by_name_cache
    if _by_id_cache is None:
        _by_id_cache = {}
        _by_name_cache = {}
        for entry in all_comps():
            _by_id_cache[entry.com_id] = entry
            _by_name_cache.setdefault(entry.name.lower(), entry)
    return _by_id_cache, _by_name_cache


_name_by_id = None


def by_id(com_id):
    index, _names = _index()
    return index.get(com_id)


def name_by_id(com_id):
    """Just the name: a flat id->name index, built once (UI hot path)."""
    global _name_by_id
    if _name_by_id is None:
        _name_by_id = {row["id"]: row["name"] for row in base.load_table("comps") if row}
    return _name_by_id.get(com_id)


def by_name(name):
    _ids, names = _index()
    found = names.get(name.lower())
    return [found] if found is not None else []


def com_id(name, default=None):
    found = by_name(name)
    return found[0].com_id if found else default
