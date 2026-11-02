import unittest
from types import SimpleNamespace


import gamedata as knowledge

from scripts.melee import run


ALL_SCIMITARS = {
    "bronze_scimitar",
    "iron_scimitar",
    "steel_scimitar",
    "black_scimitar",
    "mithril_scimitar",
    "adamant_scimitar",
    "rune_scimitar",
}


def levels(attack=1, strength=1, defence=1):
    return {"attack": attack, "defence": defence, "strength": strength}


class GearTest(unittest.TestCase):
    def test_best_scimitar_for_level(self):
        expected = {
            1: "iron_scimitar",
            5: "steel_scimitar",
            10: "black_scimitar",
            20: "mithril_scimitar",
            30: "adamant_scimitar",
            40: "rune_scimitar",
        }
        for level, name in expected.items():
            plan = run.gear_plan(ALL_SCIMITARS, levels(level, level, level))
            self.assertEqual(plan[3][0].name, name, "attack %d" % level)

    def test_scimitar_beats_2h(self):
        plan = run.gear_plan({"rune_scimitar", "rune_2h_sword"}, levels(40, 40, 40))
        self.assertEqual(plan[3][0].name, "rune_scimitar")
        self.assertFalse(knowledge.covers_slot(plan[3][0], 5))

    def test_requirements_gate_armour(self):
        plan = run.gear_plan({"rune_full_helm", "iron_full_helm"}, levels(1, 1, 1))
        self.assertEqual(plan[0][0].name, "iron_full_helm")

    def test_boots_and_gloves_use_their_slots(self):
        plan = run.gear_plan({"leather_boots", "leather_gloves"}, levels())
        self.assertEqual(plan[9][0].name, "leather_gloves")
        self.assertEqual(plan[10][0].name, "leather_boots")


class SpotTest(unittest.TestCase):
    def test_best_spot_follows_level(self):
        self.assertEqual(knowledge.best_spot(1).key, "cows")
        self.assertEqual(knowledge.best_spot(10).key, "men")
        self.assertEqual(knowledge.best_spot(25).key, "dark_wizards")
        self.assertEqual(knowledge.best_spot(40).key, "moss_giants")

    def test_underground_spawns_match_entrance(self):
        giants = knowledge.spot_by_key("hill_giants")
        spawns = run.npc_spawns(giants)
        self.assertTrue(spawns)
        self.assertTrue(all(spawn.z >= 9000 for spawn in spawns))
        cows = knowledge.spot_by_key("cows")
        self.assertTrue(all(spawn.z < 9000 for spawn in run.npc_spawns(cows)))

    def test_pick_cluster_near_lumbridge(self):
        bot = SimpleNamespace(world=SimpleNamespace(x=3222, z=3218, level=0))
        target = run.pick_cluster(bot, knowledge.spot_by_key("cows"))
        self.assertIsNotNone(target)
        self.assertGreaterEqual(target[2], 5)

    def test_clusters_split_far_apart_spawns(self):
        spawns = [
            SimpleNamespace(x=3000, z=3000),
            SimpleNamespace(x=3005, z=3005),
            SimpleNamespace(x=3100, z=3100),
        ]
        self.assertEqual(len(run.clusters(spawns)), 2)


class StyleAndFoodTest(unittest.TestCase):
    def test_slash_rotation(self):
        self.assertEqual(
            run.style_plan("weapon_slash"),
            {
                "attack": "combat_hacksword:hack0",
                "strength": "combat_hacksword:hack1",
                "defence": "combat_hacksword:hack3",
            },
        )

    def test_unarmed_fallback(self):
        self.assertEqual(run.style_plan(None)["attack"], "combat_unarmed:unarmed0")

    def test_best_food(self):
        self.assertEqual(knowledge.best_food({"lobster", "swordfish", "trout"}), ("swordfish", 14))
        self.assertIsNone(knowledge.best_food({"bones", "logs"}))

    def test_bury_bones_frees_the_slot(self):
        bones = next(obj.obj_id for obj in knowledge.obj_by_name("bones"))
        food = next(obj.obj_id for obj in knowledge.obj_by_name("swordfish"))

        class FakeBot:
            def __init__(self):
                self.slots = {0: (bones, 1), 1: (food, 1)}

            def backpack(self):
                return 3214, dict(self.slots)

            def op_held(self, op, obj, slot, com):
                self.slots.pop(slot, None)

            def pump(self, count=1):
                pass

        bot = FakeBot()
        run.bury_bones(bot)
        self.assertEqual(bot.slots, {1: (food, 1)})

    def test_parse_args(self):
        spots, kills, ticks, no_bank = run.parse_args(["run.py", "admin", "cows", "--kills", "3", "guards", "--ticks", "200"])
        self.assertEqual(spots, ["cows", "guards"])
        self.assertEqual(kills, 3)
        self.assertEqual(ticks, 200)
        self.assertFalse(no_bank)
        _spots, _kills, _ticks, no_bank = run.parse_args(["run.py", "admin", "cows", "--no-bank"])
        self.assertTrue(no_bank)


if __name__ == "__main__":
    unittest.main()
