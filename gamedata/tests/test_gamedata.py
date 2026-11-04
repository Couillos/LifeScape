"""Known-good vectors: if any of these break, the dump or the code regressed."""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import gamedata as knowledge


class GamedataTest(unittest.TestCase):
    def test_bronze_dagger(self):
        found = knowledge.obj_by_name("bronze_dagger")
        self.assertEqual(len(found), 1)
        dagger = found[0]
        self.assertEqual(dagger.obj_id, 1205)
        self.assertEqual(dagger.price, 10)
        self.assertEqual(dagger.alch_high, 6)
        self.assertEqual(dagger.alch_low, 4)

    def test_airrune(self):
        found = knowledge.obj_by_name("airrune")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].obj_id, 556)

    def test_cook_instructor(self):
        found = knowledge.npc_by_name("newbie_cook_instructor")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].npc_id, 942)
        self.assertIn("Talk-to", found[0].ops)

    def test_furnace_footprint(self):
        found = knowledge.loc_by_name("newbiefurnace")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].loc_id, 3044)
        self.assertEqual((found[0].width, found[0].length), (3, 3))

    def test_known_comps(self):
        self.assertEqual(knowledge.com_id("magic:wind_strike"), 1152)
        self.assertEqual(knowledge.com_id("controls:com_5"), 153)
        self.assertEqual(knowledge.com_id("smithing:column1"), 1119)
        self.assertEqual(knowledge.com_id("player_kit:accept"), 3651)

    def test_wind_strike_facts(self):
        found = knowledge.spell_by_name("wind_strike")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].level, 1)
        self.assertEqual(found[0].runes, {"mindrune": 1, "airrune": 1})
        self.assertEqual(found[0].maxhit, 2)
        self.assertEqual(found[0].com, "magic:wind_strike")

    def test_smelt_bronze_recipe(self):
        found = knowledge.recipes_making("bronze_bar", "smelt")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].inputs, {"copper_ore": 1, "tin_ore": 1})
        self.assertEqual(found[0].outputs, {"bronze_bar": 1})

    def test_smith_bronze_dagger_recipe(self):
        found = knowledge.recipes_making("bronze_dagger", "smith")
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].inputs, {"bronze_bar": 1})

    def test_axeshop(self):
        found = knowledge.shop_by_name("axeshop")
        self.assertEqual(len(found), 1)
        self.assertTrue(any(item.obj == "bronze_axe" for item in found[0].stocks))
        keepers = knowledge.keeper_for_npc("bob")
        self.assertTrue(any(keeper["shop"] == "axeshop" for keeper in keepers))

    def test_cook_quest_reqs(self):
        found = knowledge.quest_by_name("quest_cook")
        self.assertEqual(len(found), 1)
        objs = {check.obj for check in found[0].item_checks}
        self.assertTrue({"pot_flour", "egg", "bucket_milk"} <= objs)
        self.assertTrue(any(reward.skill == "cooking" for reward in found[0].xp_rewards))

    def test_survival_static_spawn(self):
        found = [(spawn.x, spawn.z) for spawn in knowledge.all_npc_spawns(943)]
        self.assertIn((3103, 3095), found)

    def test_spawns_inside_npcs_json(self):
        cook = knowledge.npc_by_id(942)
        self.assertIn((3075, 3085), [(spawn.x, spawn.z) for spawn in cook.spawns])

    def test_members_flags(self):
        self.assertFalse(knowledge.obj_by_name("airrune")[0].members)
        # Rev 274 content has no members flag on rune platebody (faithful dump).
        self.assertFalse(knowledge.obj_by_name("rune_platebody")[0].members)
        self.assertFalse(knowledge.obj_by_name("bronze_dagger")[0].members)
        self.assertFalse(knowledge.npc_by_name("hans")[0].members)
        self.assertFalse(knowledge.spell_by_name("wind_strike")[0].members)
        self.assertTrue(knowledge.spell_by_name("wind_wave")[0].members)
        self.assertFalse(knowledge.spell_by_name("telekinetic_grab")[0].members)
        self.assertTrue(knowledge.spell_by_name("camelot_teleport")[0].members)
        self.assertFalse(knowledge.shop_by_name("axeshop")[0].members)
        self.assertIsNone(knowledge.quest_by_name("quest_cook")[0].members)
        self.assertFalse(knowledge.recipes_making("bronze_dagger", "smith")[0].members)

    def test_spawn_members_flags(self):
        hans = [spawn for spawn in knowledge.npc_spawns_near(3207, 3233, 0, 5) if spawn.npc_id == 0]
        self.assertTrue(hans)
        self.assertFalse(any(spawn.members for spawn in hans))
        knives = knowledge.obj_spawns_near(3224, 3202, 0, 5)
        self.assertTrue(knives)
        self.assertFalse(any(spawn.members for spawn in knives))

    def test_bank_members_follow_location(self):
        draynor = [bank for bank in knowledge.all_banks() if abs(bank.x - 3091) + abs(bank.z - 3243) <= 5]
        self.assertTrue(draynor)
        self.assertFalse(any(bank.members for bank in draynor))
        far = knowledge.nearest_bank(3222, 3222, 0)
        self.assertIsNotNone(far)
        self.assertFalse(far.members)
        self.assertTrue(any(bank.members for bank in knowledge.all_banks()))

    def test_tree_table_and_spots(self):
        oak_tier = next(tier for tier in knowledge.all_tiers() if tier.product == "oak_logs")
        self.assertEqual((oak_tier.level, oak_tier.exp, oak_tier.product_members), (15, 375, False))
        self.assertEqual(knowledge.tier_for_level(14).product, "logs")
        self.assertEqual(knowledge.tier_for_level(15).product, "oak_logs")
        self.assertEqual(knowledge.tier_for_level(99).product, "magic_logs")
        spots = knowledge.loc_spawns_near(("oaktree",), 3192, 3213, 0, radius=5)
        self.assertTrue(spots)
        self.assertTrue(all(spot.members is None for spot in spots))

    def test_npc_hunt_fields(self):
        young = knowledge.npc_by_name("young_dark_wizard")[0]
        self.assertTrue(young.hunts_players)
        self.assertEqual((young.huntrange, young.vislevel), (3, 7))
        goblin = knowledge.npc_by_name("goblin")[0]
        self.assertFalse(goblin.hunts_players)
        self.assertEqual(goblin.huntrange, 0)
        hunters = [entry for entry in knowledge.all_npcs() if entry is not None and entry.hunts_players]
        self.assertTrue(len(hunters) > 10)
        self.assertTrue(all(entry.huntrange > 0 for entry in hunters))

    def test_rock_table_and_spots(self):
        copper_tier = next(tier for tier in knowledge.all_rock_tiers() if tier.product == "copper_ore")
        self.assertEqual((copper_tier.level, copper_tier.exp, copper_tier.product_members), (1, 175, False))
        self.assertEqual(knowledge.rock_tier_for_level(15).product, "iron_ore")
        self.assertEqual(knowledge.rock_tier_for_level(30).product, "coal")
        spots = knowledge.loc_spawns_near(("copperrock1", "copperrock2"), 3299, 3015, 0, radius=30)
        self.assertTrue(spots)
        self.assertTrue(all(spot.members is None for spot in spots))

    def test_fish_table_and_spots(self):
        shrimp = next(tier for tier in knowledge.all_fish_tiers() if tier.products[0] == "raw_shrimp")
        self.assertEqual((shrimp.category, shrimp.op, shrimp.level, shrimp.tool, shrimp.bait), ("saltfish", 1, 1, "net", None))
        self.assertEqual(shrimp.exp, (100, 400))
        trout = next(tier for tier in knowledge.all_fish_tiers() if tier.products[0] == "raw_trout")
        self.assertEqual((trout.level, trout.tool, trout.bait), (20, "fly_fishing_rod", "feather"))
        lobster = next(tier for tier in knowledge.all_fish_tiers() if tier.products[0] == "raw_lobster")
        self.assertEqual((lobster.category, lobster.op, lobster.level), ("rarefish", 1, 40))
        self.assertEqual(knowledge.best_fish_tier(1).products[0], "raw_shrimp")
        self.assertEqual(knowledge.best_fish_tier(25).products[0], "raw_pike")
        self.assertTrue(all(tier.product_members is False for tier in knowledge.all_fish_tiers()))
        obj_names = {entry.name for entry in knowledge.all_objs() if entry.name}
        for tier in knowledge.all_fish_tiers():
            self.assertIn(tier.tool, obj_names)
            for product in tier.products:
                self.assertIn(product, obj_names)
        salt_ids = knowledge.spot_npc_ids("saltfish")
        self.assertTrue(len(salt_ids) > 5)
        draynor = [npc_id for npc_id in salt_ids if any((spawn.x, spawn.z) == (3085, 3230) for spawn in knowledge.all_npc_spawns(npc_id))]
        self.assertTrue(draynor)

    def test_tables_non_empty(self):
        self.assertTrue(len(knowledge.all_objs()) > 3000)
        self.assertTrue(len(knowledge.all_npcs()) > 1000)
        self.assertTrue(len(knowledge.all_locs()) > 4000)
        self.assertTrue(len(knowledge.all_comps()) > 1000)


if __name__ == "__main__":
    unittest.main()
