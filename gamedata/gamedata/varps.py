"""Player variables (varps): id, name and persistence.

Generated from the server's packed varp config. Quest completion lives in
permanent varps, which the server saves but does not transmit to the client.
"""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class Varp:
    varp_id: int
    name: str
    scope: int
    transmit: bool


_all = None
_by_name = None


def all_varps():
    global _all
    if _all is None:
        _all = [
            Varp(
                varp_id=row["id"],
                name=row["name"],
                scope=row["scope"],
                transmit=bool(row["transmit"]),
            )
            for row in base.load_table("varps")
        ]
    return _all


def _index():
    global _by_name
    if _by_name is None:
        _by_name = {varp.name.lower(): varp for varp in all_varps()}
    return _by_name


def by_name(name):
    return _index().get(name.lower())


def id_by_name(name):
    varp = by_name(name)
    return varp.varp_id if varp is not None else None
