"""Bank access points: booth loc spawns with location-derived members.

Locs carry no server members flag, so a booth counts as members-only when
it stands outside the F2P map (computed by the dumper from free2play.csv,
level ignored like the server). Trees and other locs keep members null;
use their product obj flag instead.
"""

from dataclasses import dataclass

from gamedata import locs as locs_module

BANK_LOC_NAMES = ("bankbooth",)


@dataclass(frozen=True)
class Bank:
    x: int
    z: int
    level: int
    loc_id: int
    name: str
    members: bool = False


def all_banks():
    wanted = set(BANK_LOC_NAMES)
    found = []
    for entry in locs_module.all_locs():
        if (entry.name or "").lower() not in wanted:
            continue
        for spot in entry.spawns:
            found.append(Bank(x=spot.x, z=spot.z, level=spot.level, loc_id=entry.loc_id, name=entry.name or "", members=bool(spot.members)))
    return found


def nearest_bank(x, z, level=0):
    best = None
    for bank in all_banks():
        if bank.level != level:
            continue
        dist = abs(bank.x - x) + abs(bank.z - z)
        if best is None or dist < best[0]:
            best = (dist, bank)
    return best[1] if best is not None else None
