import unittest

import gamedata as knowledge

from orchestration import naming


class NamingTest(unittest.TestCase):
    def test_resolve_state_fills_names_from_ids(self):
        state = {
            "modal": ["side", 2458],
            "hint": ["npc", 42],
            "npcs": [{"nid": 42, "type": 7, "name": ""}],
            "locs": [{"id": 3014, "name": ""}],
            "objs": [{"id": 995, "name": ""}],
        }
        naming.resolve_state(state)
        self.assertEqual(state["modal_name"], "Logout:try logout")
        self.assertEqual(state["npcs"][0]["name"], naming.humanize(knowledge.npc_name_by_id(7)))
        self.assertEqual(state["locs"][0]["name"], naming.humanize(knowledge.loc_name_by_id(3014)))
        self.assertEqual(state["objs"][0]["name"], naming.humanize(knowledge.obj_name_by_id(995)))
        self.assertTrue(state["hint_name"])

    def test_hint_coord_and_unknown_npc(self):
        self.assertEqual(naming.resolve_state({"hint": ["coord", 3200, 3201]})["hint_name"], "Tile 3200,3201")
        self.assertEqual(naming.resolve_state({"hint": ["npc", 999]})["hint_name"], "Npc 999")

    def test_humanize_words(self):
        self.assertEqual(naming.humanize("young_dark_wizard"), "Young dark wizard")
        self.assertEqual(naming.humanize("olivia_king"), "Olivia king")
        self.assertEqual(naming.humanize("0_48_50_saltfish"), "Saltfish")

    def test_cosmetic_scenery_is_hidden(self):
        state = {
            "locs": [
                {"id": 1257, "name": "", "act": False},  # grass_light_1
                {"id": 3014, "name": "", "act": True},   # newbie_door1
            ]
        }
        naming.resolve_state(state)
        self.assertEqual([loc["name"] for loc in state["locs"]], ["Newbie door1"])

    def test_flowerbeds_move_to_flora_instead_of_vanishing(self):
        state = {
            "locs": [
                {"id": 1257, "name": "", "act": False},  # grass_light_1
                {"id": 1190, "name": "", "act": False},  # sunflower
            ]
        }
        naming.resolve_state(state)
        self.assertEqual(state["locs"], [])
        self.assertEqual(len(state["flora"]), 1)
        self.assertEqual(state["flora"][0]["name"], "Sunflower")

    def test_account_gets_a_display_name(self):
        state = naming.resolve_state({"account": "olivia_king"})
        self.assertEqual(state["account_name"], "Olivia king")

    def test_gamedata_lists_are_cached(self):
        self.assertIs(knowledge.all_npcs(), knowledge.all_npcs())
        self.assertIs(knowledge.all_locs(), knowledge.all_locs())
        self.assertIs(knowledge.all_objs(), knowledge.all_objs())


if __name__ == "__main__":
    unittest.main()
