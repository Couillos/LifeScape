"""Typed quest requirement checks over a player snapshot.

Reasons are explicit types so the orchestrator never parses prose:
SkillTooLow, QuestPointsTooLow, ItemMissing and QuestNotCompleted. Each
reason serialises with a ``type`` discriminator plus the raw numbers.
"""

from dataclasses import dataclass
from typing import ClassVar

SKILL_INDEX = {
    "attack": 0,
    "defence": 1,
    "strength": 2,
    "hitpoints": 3,
    "ranged": 4,
    "prayer": 5,
    "magic": 6,
    "cooking": 7,
    "woodcutting": 8,
    "fletching": 9,
    "fishing": 10,
    "firemaking": 11,
    "crafting": 12,
    "smithing": 13,
    "mining": 14,
    "herblore": 15,
    "agility": 16,
    "thieving": 17,
    "slayer": 18,
    "farming": 19,
    "runecraft": 20,
}


@dataclass(frozen=True)
class PlayerState:
    levels: dict
    inventory: dict
    worn: dict
    bank: dict
    varps: dict
    quest_points: int
    combat_level: int = 0


@dataclass(frozen=True)
class SkillTooLow:
    TYPE: ClassVar[str] = "skill"
    skill: str
    required: int
    current: int

    def as_dict(self):
        return {"type": self.TYPE, "skill": self.skill, "required": self.required, "current": self.current}


@dataclass(frozen=True)
class CombatLevelTooLow:
    TYPE: ClassVar[str] = "combat"
    required: int
    current: int
    soft: bool = False

    def as_dict(self):
        return {"type": self.TYPE, "required": self.required, "current": self.current, "soft": self.soft}


@dataclass(frozen=True)
class QuestPointsTooLow:
    TYPE: ClassVar[str] = "quest_points"
    required: int
    current: int

    def as_dict(self):
        return {"type": self.TYPE, "required": self.required, "current": self.current}


@dataclass(frozen=True)
class ItemMissing:
    TYPE: ClassVar[str] = "item"
    item: str
    name: str
    required: int
    inventory: int
    worn: int
    bank: int
    needs_worn: bool = False

    @property
    def owned(self):
        return self.inventory + self.worn + self.bank

    def as_dict(self):
        return {
            "type": self.TYPE,
            "item": self.item,
            "name": self.name,
            "required": self.required,
            "owned": self.owned,
            "inventory": self.inventory,
            "worn": self.worn,
            "bank": self.bank,
            "needs_worn": self.needs_worn,
        }


@dataclass(frozen=True)
class NavigationBlocked:
    TYPE: ClassVar[str] = "navigation"
    detail: str

    def as_dict(self):
        return {"type": self.TYPE, "detail": self.detail}


@dataclass(frozen=True)
class QuestNotCompleted:
    TYPE: ClassVar[str] = "quest"
    quest: str
    name: str

    def as_dict(self):
        return {"type": self.TYPE, "quest": self.quest, "name": self.name}


@dataclass(frozen=True)
class QuestReport:
    quest: str
    name: str
    met: bool
    completed: bool
    reasons: tuple

    def as_dict(self):
        return {
            "quest": self.quest,
            "name": self.name,
            "met": self.met,
            "completed": self.completed,
            "reasons": [reason.as_dict() for reason in self.reasons],
        }

    def summary(self):
        if self.completed:
            return "%s: already completed" % self.name

        def describe(reason):
            if reason.TYPE == "skill":
                return "%s %d/%d" % (reason.skill, reason.current, reason.required)
            if reason.TYPE == "combat":
                return "%scombat %d/%d" % ("recommended " if reason.soft else "", reason.current, reason.required)
            if reason.TYPE == "quest_points":
                return "quest points %d/%d" % (reason.current, reason.required)
            if reason.TYPE == "item":
                return "%s %d/%d (inv %d, worn %d, bank %d)%s" % (
                    reason.item, reason.owned, reason.required, reason.inventory, reason.worn, reason.bank,
                    " must be worn" if reason.needs_worn else "",
                )
            if reason.TYPE == "quest":
                return "quest %s" % reason.name
            if reason.TYPE == "navigation":
                return "navigation: %s" % reason.detail
            return reason.TYPE

        if self.met:
            soft = [reason for reason in self.reasons if getattr(reason, "soft", False)]
            if not soft:
                return "%s: requirements met" % self.name
            return "%s: requirements met (%s)" % (self.name, "; ".join(describe(reason) for reason in soft))
        return "%s: missing %s" % (self.name, "; ".join(describe(reason) for reason in self.reasons))


def _completed(completion_varps, varps):
    return any(varps.get(varp_id) == value for varp_id, value in completion_varps)


def check(requirements, state):
    reasons = []
    for skill in requirements.skills:
        current = state.levels.get(skill.skill, 0)
        if current < skill.level:
            reasons.append(SkillTooLow(skill=skill.skill, required=skill.level, current=current))
    if requirements.combat_level and state.combat_level < requirements.combat_level:
        reasons.append(CombatLevelTooLow(required=requirements.combat_level, current=state.combat_level))
    if requirements.recommended_combat and state.combat_level < requirements.recommended_combat:
        reasons.append(CombatLevelTooLow(
            required=requirements.recommended_combat, current=state.combat_level, soft=True
        ))
    if requirements.quest_points and state.quest_points < requirements.quest_points:
        reasons.append(QuestPointsTooLow(required=requirements.quest_points, current=state.quest_points))
    for item in requirements.items:
        in_inventory = state.inventory.get(item.obj_id, 0)
        worn = state.worn.get(item.obj_id, 0)
        in_bank = state.bank.get(item.obj_id, 0)
        # worn_only means the solve must end up wearing it (it equips the
        # kit itself): carried anywhere suffices to start, since a pack or
        # bank item can still be worn in time.
        satisfied = in_inventory + worn + in_bank >= item.count
        if not satisfied:
            reasons.append(ItemMissing(
                item=item.item,
                name=item.name,
                required=item.count,
                inventory=in_inventory,
                worn=worn,
                bank=in_bank,
                needs_worn=item.worn_only,
            ))
    for dependency in requirements.dependencies:
        if not _completed(dependency.completion_varps, state.varps):
            reasons.append(QuestNotCompleted(quest=dependency.key, name=dependency.name))
    hard_reasons = [reason for reason in reasons if not getattr(reason, "soft", False)]
    return QuestReport(
        quest=requirements.key,
        name=requirements.name,
        met=not hard_reasons,
        completed=_completed(requirements.completion_varps, state.varps),
        reasons=tuple(reasons),
    )
