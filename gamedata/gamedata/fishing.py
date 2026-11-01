"""Fishing table: spot categories, level reqs, tools, bait, products, XP.

Authoritative source is the server scripts (no dbtable exists for
fishing, unlike woodcutting/mining):
- content/scripts/skill_fishing/scripts/fishing_spots/*.rs2 (op handlers:
  level gates, check_fish_equipment(tool), fish_roll products),
- content/scripts/skill_fishing/configs/fishing.struct (productexp),
- content/scripts/skill_fishing/configs/fishing_equipment.struct
  (tool -> bait).

Only free-to-play rows are listed (memberfish/big-net/shark and the
quest/lava/slimey spots are skipped). XP is raw (tenths, as the
server stores it). Spots are NPCs (display name "Fishing spot");
`category` matches the tail of their debugnames (e.g.
`0_48_50_saltfish`) and the `_<category>` script targets.
"""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class FishTier:
    category: str
    op: int
    action: str
    level: int
    tool: str
    bait: str | None
    products: tuple
    second_level: int
    exp: tuple
    product_members: bool = False


_FISH_TIERS = (
    # saltfish.rs2: opnpc1 shrimp, opnpc3 sardine; anchovies/herring
    # join the same roll at 15/10.
    FishTier("saltfish", 1, "Net", 1, "net", None, ("raw_shrimp", "raw_anchovies"), 15, (100, 400)),
    FishTier("saltfish", 3, "Bait", 5, "fishing_rod", "fishing_bait", ("raw_sardine", "raw_herring"), 10, (200, 300)),
    # freshfish.rs2: opnpc1 trout/salmon (30), opnpc3 pike.
    FishTier("freshfish", 1, "Lure", 20, "fly_fishing_rod", "feather", ("raw_trout", "raw_salmon"), 30, (500, 700)),
    FishTier("freshfish", 3, "Bait", 25, "fishing_rod", "fishing_bait", ("raw_pike",), 0, (600,)),
    # rarefish.rs2: opnpc1 lobster, opnpc3 tuna/swordfish (50).
    FishTier("rarefish", 1, "Cage", 40, "lobster_pot", None, ("raw_lobster",), 0, (900,)),
    FishTier("rarefish", 3, "Harpoon", 35, "harpoon", None, ("raw_tuna", "raw_swordfish"), 50, (800, 1000)),
)


def all_fish_tiers():
    return list(_FISH_TIERS)


def affordable_tiers(fishing_level):
    return [tier for tier in _FISH_TIERS if tier.level <= fishing_level]


def best_fish_tier(fishing_level):
    best = None
    for tier in _FISH_TIERS:
        if tier.level <= fishing_level and (best is None or tier.level > best.level):
            best = tier
    return best


def spot_npc_ids(category):
    """NPC ids whose debugname ends with _<category> (static spawn holders)."""
    tail = "_" + category
    return tuple(
        npc_id for npc_id, row in enumerate(base.load_table("npcs")) if row and (row.get("name") or "").endswith(tail)
    )
