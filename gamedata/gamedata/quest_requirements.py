"""Typed requirements for the free-to-play quests, read from the server code.

Skill, quest-point and item gates are the predicates the server quest
scripts enforce (``stat_base``, ``%qp``, ``inv_total``). Item names are
debugnames resolved against the generated object table; completion varps
are resolved against the generated varp table.

Dependencies between F2P quests are empty in revision 274: the only
cross-quest gates are quest points, modelled separately. The dependency
type is kept for members quests and for the checker.
"""

from dataclasses import dataclass

from gamedata import objs, varps


@dataclass(frozen=True)
class SkillRequirement:
    skill: str
    level: int


@dataclass(frozen=True)
class ItemRequirement:
    item: str
    obj_id: int
    name: str
    count: int
    worn_only: bool = False


@dataclass(frozen=True)
class QuestDependency:
    key: str
    name: str
    completion_varps: tuple


@dataclass(frozen=True)
class QuestRequirements:
    key: str
    name: str
    members: bool
    completion_varps: tuple
    skills: tuple = ()
    quest_points: int = 0
    items: tuple = ()
    dependencies: tuple = ()
    combat_level: int = 0
    recommended_combat: int = 0


def _skill(name, level):
    return SkillRequirement(skill=name, level=level)


def _item(name, count=1, worn_only=False):
    found = objs.by_name(name)
    if not found:
        raise KeyError("unknown quest item %r" % name)
    entry = found[0]
    return ItemRequirement(item=name, obj_id=entry.obj_id, name=entry.name or name, count=count, worn_only=worn_only)


def _completion(*pairs):
    resolved = []
    for name, value in pairs:
        varp = varps.by_name(name)
        if varp is None:
            raise KeyError("unknown quest varp %r" % name)
        resolved.append((varp.varp_id, value))
    return tuple(resolved)


F2P_QUESTS = (
    QuestRequirements(
        key="blackknight",
        name="Black Knight's Fortress",
        members=False,
        completion_varps=_completion(("spy", 4)),
        quest_points=12,
        items=(_item("iron_chainbody", worn_only=True), _item("bronze_med_helm", worn_only=True), _item("cabbage")),
    ),
    QuestRequirements(
        key="cook",
        name="Cook's Assistant",
        members=False,
        completion_varps=_completion(("cookquest", 2)),
        items=(_item("bucket_milk"), _item("egg"), _item("pot_flour")),
    ),
    QuestRequirements(
        key="demon",
        name="Demon Slayer",
        members=False,
        completion_varps=_completion(("demonstart", 30)),
        items=(_item("coins"), _item("bucket_water"), _item("bones", 25)),
    ),
    QuestRequirements(
        key="doric",
        name="Doric's Quest",
        members=False,
        completion_varps=_completion(("doricquest", 100)),
        items=(_item("clay", 6), _item("copper_ore", 4), _item("iron_ore", 2)),
    ),
    QuestRequirements(
        key="dragon",
        name="Dragon Slayer",
        members=False,
        completion_varps=_completion(("dragonquest", 10)),
        quest_points=32,
        items=(
            _item("coins", 12000),
            _item("woodplank", 3),
            _item("nails", 12),
            _item("hammer"),
            _item("wizards_mind_bomb"),
            _item("bowl_unfired"),
            _item("lobster_pot"),
            _item("silk"),
            _item("antidragonbreathshield"),
        ),
    ),
    QuestRequirements(
        key="haunted",
        name="Ernest the Chicken",
        members=False,
        completion_varps=_completion(("haunted", 3)),
        items=(
            _item("spade"),
            _item("fish_food"),
            _item("poison"),
            _item("oil_can"),
            _item("pressure_gauge"),
            _item("rubber_tube"),
        ),
    ),
    QuestRequirements(
        key="gobdip",
        name="Goblin Diplomacy",
        members=False,
        completion_varps=_completion(("goblinquest", 6)),
        items=(_item("goblin_armour_orange"), _item("goblin_armour_darkblue"), _item("goblin_armour")),
    ),
    QuestRequirements(
        key="squire",
        name="The Knight's Sword",
        members=False,
        completion_varps=_completion(("squire", 7)),
        skills=(_skill("mining", 10),),
        # Journal: "an advantage if I had a combat level of 20" (not a gate).
        recommended_combat=20,
        items=(_item("redberry_pie"), _item("blurite_ore"), _item("iron_bar", 2)),
    ),
    QuestRequirements(
        key="imp",
        name="Imp Catcher",
        members=False,
        completion_varps=_completion(("imp", 2)),
        items=(_item("black_bead"), _item("red_bead"), _item("white_bead"), _item("yellow_bead")),
    ),
    QuestRequirements(
        key="hunt",
        name="Pirate's Treasure",
        members=False,
        completion_varps=_completion(("hunt", 4)),
        # No rum: it is bought on Karamja mid-quest and customs seizes
        # any carried bottle, so requiring it upfront can never pass.
        items=(_item("banana", 10),),
    ),
    QuestRequirements(
        key="prince",
        name="Prince Ali Rescue",
        members=False,
        completion_varps=_completion(("princequest", 110)),
        items=(
            _item("princeskey"),
            # No rope: it is consumed tying Keli mid-quest (like beer).
            # No beer: all 3 are drunk by the guard mid-quest, so requiring
            # them upfront can never pass once Joe is done (like hunt rum).
            _item("skinpaste"),
            _item("pink_skirt"),
            _item("blondwig"),
        ),
    ),
    QuestRequirements(
        key="priest",
        name="The Restless Ghost",
        members=False,
        completion_varps=_completion(("prieststart", 5)),
    ),
    QuestRequirements(
        key="romeojuliet",
        name="Romeo & Juliet",
        members=False,
        completion_varps=_completion(("rjquest", 100)),
        items=(_item("cadavaberries"),),
    ),
    QuestRequirements(
        key="runemysteries",
        name="Rune Mysteries",
        members=False,
        completion_varps=_completion(("runemysteries", 6)),
    ),
    QuestRequirements(
        key="sheep",
        name="Sheep Shearer",
        members=False,
        completion_varps=_completion(("sheep", 22)),
        items=(_item("ball_of_wool", 20),),
    ),
    QuestRequirements(
        key="blackarmgang",
        name="Shield of Arrav",
        members=False,
        completion_varps=_completion(("blackarmgang", 4), ("phoenixgang", 10)),
        items=(_item("arravshield1"), _item("arravshield2")),
    ),
    QuestRequirements(
        key="vampire",
        name="Vampire Slayer",
        members=False,
        completion_varps=_completion(("vampire", 3)),
        items=(_item("stake"), _item("garlic"), _item("hammer")),
    ),
    QuestRequirements(
        key="hetty",
        name="Witch's Potion",
        members=False,
        completion_varps=_completion(("hetty", 3)),
        items=(_item("onion"), _item("rats_tail"), _item("burnt_meat"), _item("eye_of_newt")),
    ),
)

_BY_KEY = {quest.key: quest for quest in F2P_QUESTS}


def all_f2p_quests():
    return F2P_QUESTS


def by_key(key):
    quest = _BY_KEY.get(key)
    if quest is None:
        raise KeyError("unknown F2P quest %r" % key)
    return quest
