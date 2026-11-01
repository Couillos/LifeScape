"""Typed game knowledge: objects, NPCs, scenery, components, shops,
spells, craft recipes, quests, ground and NPC spawns.

All data comes from generated JSON (see ../AGENTS.md); this package only
offers typed, documented accessors over it.
"""

from gamedata import base, banks, combat_spots, comps, equipment, fishing, food, locs, npcs, objs, quests, varps, quest_requirements, recipes, rocks, shops, spawns, spells, trees
from gamedata import base
from gamedata.combat_spots import CombatSpot, Entrance, all_spots, best_spot, by_key as spot_by_key
from gamedata.equipment import Wearable, all_wearables, by_name as wearable_by_name, covers_slot, requirements_met
from gamedata.fishing import FishTier, affordable_tiers, all_fish_tiers, best_fish_tier, spot_npc_ids
from gamedata.food import all_foods, best_food, heal as food_heal
from gamedata.rocks import RockTier, all_rock_tiers, rock_tier_for_level
from gamedata.banks import Bank, all_banks, nearest_bank
from gamedata.comps import Comp, all_comps, by_id as comp_by_id, by_name as comp_by_name, com_id, name_by_id as comp_name_by_id
from gamedata.locs import Loc, SpawnLoc, all_locs, by_id as loc_by_id, by_name as loc_by_name, search as loc_search, loc_spawns_near, name_by_id as loc_name_by_id
from gamedata.npcs import Npc, SpawnPoint, all_npcs, by_id as npc_by_id, by_name as npc_by_name, search as npc_search, name_by_id as npc_name_by_id
from gamedata.objs import Obj, all_objs, by_id as obj_by_id, by_name as obj_by_name, search as obj_search, name_by_id as obj_name_by_id
from gamedata.quests import Quest, SkillReq, ItemCheck, XpReward, all_quests, by_name as quest_by_name
from gamedata.quest_requirements import (
    ItemRequirement,
    QuestDependency,
    QuestRequirements,
    SkillRequirement,
    all_f2p_quests,
    by_key as quest_requirements_by_key,
)
from gamedata.varps import Varp, all_varps, by_name as varp_by_name, id_by_name as varp_id_by_name
from gamedata.recipes import Recipe, all_recipes, making as recipes_making, using as recipes_using
from gamedata.shops import Shop, Stock, Keeper, all_shops, by_name as shop_by_name, keeper_for_npc
from gamedata.spawns import NpcSpawn, ObjSpawn, all_npc_spawns, all_obj_spawns, obj_spawns_near, npc_spawns_near
from gamedata.spells import Spell, all_spells, by_name as spell_by_name, by_com as spell_by_com
from gamedata.trees import TreeTier, all_tiers, tier_for_level

__all__ = [
    "base",
    "objs", "Obj", "all_objs", "obj_by_id", "obj_by_name", "obj_search", "obj_name_by_id",
    "npcs", "Npc", "SpawnPoint", "all_npcs", "npc_by_id", "npc_by_name", "npc_search", "npc_name_by_id",
    "locs", "Loc", "SpawnLoc", "all_locs", "loc_by_id", "loc_by_name", "loc_search", "loc_spawns_near", "loc_name_by_id",
    "banks", "Bank", "all_banks", "nearest_bank",
    "combat_spots", "CombatSpot", "Entrance", "all_spots", "best_spot", "spot_by_key",
    "equipment", "Wearable", "all_wearables", "wearable_by_name", "covers_slot", "requirements_met",
    "food", "all_foods", "best_food", "food_heal",
    "trees", "TreeTier", "all_tiers", "tier_for_level",
    "rocks", "RockTier", "all_rock_tiers", "rock_tier_for_level",
    "fishing", "FishTier", "all_fish_tiers", "affordable_tiers", "best_fish_tier", "spot_npc_ids",
    "comps", "Comp", "all_comps", "comp_by_id", "comp_by_name", "com_id", "comp_name_by_id",
    "shops", "Shop", "Stock", "Keeper", "all_shops", "shop_by_name", "keeper_for_npc",
    "spells", "Spell", "all_spells", "spell_by_name", "spell_by_com",
    "recipes", "Recipe", "all_recipes", "recipes_making", "recipes_using",
    "quests", "Quest", "SkillReq", "ItemCheck", "XpReward", "all_quests", "quest_by_name",
    "quest_requirements", "QuestRequirements", "QuestDependency", "SkillRequirement", "ItemRequirement",
    "all_f2p_quests", "quest_requirements_by_key",
    "varps", "Varp", "all_varps", "varp_by_name", "varp_id_by_name",
    "spawns", "NpcSpawn", "ObjSpawn", "all_npc_spawns", "all_obj_spawns", "obj_spawns_near", "npc_spawns_near",
]
