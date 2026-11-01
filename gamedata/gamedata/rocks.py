"""Mining table: rock loc names, level reqs, ore products, XP.

Authoritative source is mine.dbrow (parsed by the dumper into
rocks.json). XP is raw (tenths, as the server stores it).
"""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class RockTier:
    rocks: tuple
    level: int
    product: str
    exp: int
    ore_name: str = ""
    product_members: bool | None = None


def all_rock_tiers():
    return [
        RockTier(
            rocks=tuple(row.get("rocks", [])),
            level=row.get("level", 0),
            product=row.get("product", ""),
            exp=row.get("exp", 0),
            ore_name=row.get("ore_name", ""),
            product_members=row.get("product_members"),
        )
        for row in base.load_table("rocks")
        if row
    ]


def rock_tier_for_level(mining_level):
    best = None
    for tier in all_rock_tiers():
        if tier.level <= mining_level and (best is None or tier.level > best.level):
            best = tier
    return best
