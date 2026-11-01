"""Wearable stats and level requirements, from the generated equipment dump.

The dump mirrors ObjType params (attack/defence bonuses, strength) and the
levelrequire tier scripts. Slots are the server's wearpos numbers: 0 head,
2 amulet, 3 right hand, 4 body, 5 left hand, 7 legs, 9 hands, 10 feet,
12 ring. Quest-only requirements are not modelled; the script retries the
next best item when the server refuses a wear.
"""

from dataclasses import dataclass, field

from gamedata import base

STATS = {
    "attack": 0,
    "defence": 1,
    "strength": 2,
    "hitpoints": 3,
    "ranged": 4,
    "prayer": 5,
    "magic": 6,
}


@dataclass(frozen=True)
class Wearable:
    name: str
    wearpos: int
    wearpos2: int
    wearpos3: int
    category: str | None
    attack: dict = field(default_factory=dict)
    defence: dict = field(default_factory=dict)
    strength: int = 0
    prayer: int = 0
    requirements: dict = field(default_factory=dict)


_alls = None


def all_wearables():
    global _alls
    if _alls is None:
        table = base.load_table("equipment")
        _alls = {
            name: Wearable(
                name=name,
                wearpos=row["wearpos"],
                wearpos2=row.get("wearpos2", -1),
                wearpos3=row.get("wearpos3", -1),
                category=row.get("category"),
                attack=row.get("attack") or {},
                defence=row.get("defence") or {},
                strength=row.get("strength", 0) or 0,
                prayer=row.get("prayer", 0) or 0,
                requirements=row.get("requirements") or {},
            )
            for name, row in table.items()
        }
    return _alls


def by_name(name):
    return all_wearables().get(name)


def requirements_met(entry, levels):
    """levels maps stat name -> current level (see STATS)."""
    for stat, needed in entry.requirements.items():
        if stat not in STATS or levels.get(stat, 1) < needed:
            return False
    return True


def covers_slot(entry, slot):
    return slot in (entry.wearpos, entry.wearpos2, entry.wearpos3)
