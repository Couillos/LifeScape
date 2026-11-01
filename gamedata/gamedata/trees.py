"""Woodcutting table: tree loc names, level reqs, log products, XP.

Authoritative source is trees.dbrow (parsed by the dumper into
trees.json). XP is raw (tenths, as the server stores it).
"""

from dataclasses import dataclass

from gamedata import base


@dataclass(frozen=True)
class TreeTier:
    trees: tuple
    level: int
    product: str
    exp: int
    product_members: bool | None = None


def all_tiers():
    return [
        TreeTier(
            trees=tuple(row.get("trees", [])),
            level=row.get("level", 0),
            product=row.get("product", ""),
            exp=row.get("exp", 0),
            product_members=row.get("product_members"),
        )
        for row in base.load_table("trees")
        if row
    ]


def tier_for_level(woodcutting_level):
    best = None
    for tier in all_tiers():
        if tier.level <= woodcutting_level and (best is None or tier.level > best.level):
            best = tier
    return best
