"""Quests: skill/item requirements scraped from quest scripts (heuristic).

No central requirement table exists server-side; requirements live in
script predicates. The dump records every stat_base/inv_total/%qp check
and every stat_advance/send_quest_complete found per quest directory.
"""

from dataclasses import dataclass, field

from gamedata import base


@dataclass(frozen=True)
class SkillReq:
    skill: str
    op: str
    level: int


@dataclass(frozen=True)
class ItemCheck:
    container: str
    obj: str


@dataclass(frozen=True)
class XpReward:
    skill: str
    xp: int


@dataclass(frozen=True)
class Quest:
    name: str
    members: bool | None = None
    files: int = 0
    varps: tuple = ()
    skill_reqs: tuple = ()
    item_checks: tuple = ()
    qp_gates: tuple = ()
    xp_rewards: tuple = ()
    complete_raw: tuple = ()


def _row_to_quest(row):
    return Quest(
        name=row.get("quest", ""),
        files=row.get("files", 0),
        varps=tuple(row.get("varps", [])),
        members=row.get("members"),
        skill_reqs=tuple(SkillReq(skill=item.get("skill", ""), op=item.get("op", ""), level=item.get("level", 0)) for item in row.get("skill_reqs", [])),
        item_checks=tuple(ItemCheck(container=item.get("container", ""), obj=item.get("obj", "")) for item in row.get("item_checks", [])),
        qp_gates=tuple((item.get("op", ""), item.get("value", 0)) for item in row.get("qp_gates", [])),
        xp_rewards=tuple(XpReward(skill=item.get("skill", ""), xp=item.get("xp", 0)) for item in row.get("xp_rewards", [])),
        complete_raw=tuple(row.get("complete_raw", [])),
    )


def all_quests():
    return [_row_to_quest(row) for row in base.load_table("quests")]


def by_name(name):
    wanted = name.lower()
    return [entry for entry in all_quests() if entry.name.lower() == wanted]
