"""Craft recipes, normalized across dbrow tables and .struct files.

Kinds: smith (anvil), smelt (furnace), cook, leather, gem, fletch, brew.
Levels and XP are raw server values (XP is x10 in-game XP).
"""

from dataclasses import dataclass, field

from gamedata import base


@dataclass(frozen=True)
class Recipe:
    kind: str
    level: int
    xp: int
    inputs: dict
    outputs: dict
    source: str = ""
    members: bool | None = None


def all_recipes(kind=None):
    table = base.load_table("recipes")
    out = []
    for row in table:
        if kind is not None and row.get("kind") != kind:
            continue
        out.append(Recipe(
            kind=row.get("kind", ""),
            level=row.get("level", 0),
            xp=row.get("xp", 0),
            inputs=dict(row.get("inputs", {})),
            outputs=dict(row.get("outputs", {})),
            source=str((row.get("extra", {}) or {}).get("row", "")),
            members=row.get("members"),
        ))
    return out


def making(product, kind=None):
    wanted = product.lower()
    return [entry for entry in all_recipes(kind) if wanted in {name.lower() for name in entry.outputs}]


def using(obj_name, kind=None):
    wanted = obj_name.lower()
    found = []
    for entry in all_recipes(kind):
        if any(wanted == name.lower() for name in entry.inputs):
            found.append(entry)
    return found
