"""Classic F2P melee-and-loot spots, with the route to underground ones.

Each spot names the NPC debugnames to attack; actual spawn points come from
the NPC spawn dump, so the script picks the densest cluster near a bank.
Underground spots carry the trapdoor/ladder ids probed from the server
(open the surface loc, climb down, then climb the ladder back up).
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Entrance:
    """Surface tile used to reach the spot (movement links live in the SDK)."""

    surface: tuple


@dataclass(frozen=True)
class CombatSpot:
    key: str
    npc_names: tuple
    min_level: int
    max_level: int
    entrance: Entrance | None = None


EDGEVILLE_DUNGEON = Entrance(surface=(3097, 3468))
VARROCK_SEWERS = Entrance(surface=(3237, 3458))


SPOTS = (
    CombatSpot("chickens", ("chicken",), 1, 9),
    CombatSpot("cows", ("cow",), 1, 12),
    CombatSpot("al_kharid_warriors", ("al_kharid_warrior",), 8, 20),
    CombatSpot("men", ("man", "man2", "man3", "woman", "woman2", "woman3"), 8, 25),
    CombatSpot("guards", ("guard1", "guard2"), 20, 40),
    CombatSpot("dark_wizards", ("young_dark_wizard", "bearded_dark_wizard"), 20, 45),
    CombatSpot("hill_giants", ("giant",), 30, 60, EDGEVILLE_DUNGEON),
    CombatSpot("moss_giants", ("mossgiant",), 40, 80, VARROCK_SEWERS),
)

_keep = {spot.key: spot for spot in SPOTS}


def all_spots():
    return SPOTS


def by_key(key):
    return _keep.get(key)


def best_spot(melee_level):
    """Highest spot whose minimum level is reached; the first spot otherwise."""
    chosen = SPOTS[0]
    for spot in SPOTS:
        if spot.min_level <= melee_level:
            chosen = spot
    return chosen
