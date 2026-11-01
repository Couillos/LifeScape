"""Combat spells: level, runes, XP, max hit, interface component."""

from dataclasses import dataclass, field

from gamedata import base


@dataclass(frozen=True)
class Spell:
    spell: str
    name: str | None
    com: str | None
    members: bool = False
    level: int = 0
    runes: dict = field(default_factory=dict)
    xp: int = 0
    maxhit: int | None = None


def all_spells():
    table = base.load_table("spells")
    out = []
    for row in table:
        out.append(Spell(
            spell=row.get("spell", ""),
            name=row.get("name"),
            com=row.get("com"),
            members=bool(row.get("members", False)),
            level=row.get("level", 0),
            runes=dict(row.get("runes", {})),
            xp=row.get("xp", 0),
            maxhit=row.get("maxhit"),
        ))
    return out


def by_name(spell):
    wanted = spell.lower().lstrip("^")
    return [entry for entry in all_spells() if entry.spell.lower() == wanted]


def by_com(com):
    return [entry for entry in all_spells() if (entry.com or "").lower() == com.lower()]
