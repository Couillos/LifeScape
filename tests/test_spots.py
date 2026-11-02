import unittest

from lifescape import spots
from lifescape.client import Bot
import gamedata as knowledge_data


def bot_with_stats(stats, energy=100):
    bot = Bot()
    bot.world.stats = dict(stats)
    bot.world.energy = energy
    return bot


class SpotsTest(unittest.TestCase):
    def test_combat_level_matches_server_formula(self):
        self.assertEqual(spots.combat_level({0: 1, 1: 1, 2: 1, 3: 10, 4: 1, 5: 1, 6: 1}), 3)
        self.assertEqual(spots.combat_level({0: 40, 1: 40, 2: 40, 3: 40, 4: 1, 5: 1, 6: 1}), 46)
        self.assertIsNone(spots.combat_level({0: 10, 1: 10}))
        self.assertIsNone(spots.combat_level({}))

    def test_exempt_needs_more_than_twice_vislevel(self):
        self.assertTrue(spots.exempt_by_level(15, 7))
        self.assertFalse(spots.exempt_by_level(14, 7))
        self.assertFalse(spots.exempt_by_level(None, 7))
        self.assertFalse(spots.exempt_by_level(99, -1))

    def test_danger_radius_adds_wander_margin(self):
        self.assertEqual(spots.danger_radius(3, True), 8)
        self.assertEqual(spots.danger_radius(0, True), 0)
        self.assertEqual(spots.danger_radius(5, False), 0)

    def test_threats_use_chebyshev_and_level(self):
        npc = (100, 100, 0, 3, 7, True)
        self.assertEqual(len(spots.threats_at(103, 103, 0, [npc], 3)), 1)
        self.assertEqual(spots.threats_at(109, 100, 0, [npc], 3), [])
        self.assertEqual(spots.threats_at(100, 100, 1, [npc], 3), [])
        self.assertEqual(spots.threats_at(100, 100, 0, [npc], 15), [])

    def test_nearest_safe_spot_skips_threatened(self):
        npc = (100, 100, 0, 3, 7, True)
        spots_here = [(101, 101, 0), (120, 120, 0)]
        self.assertEqual(spots.nearest_safe_spot(spots_here, [npc], 3, 0, 0, 0), (120, 120, 0))
        self.assertIsNone(spots.nearest_safe_spot([(101, 101, 0)], [npc], 3, 0, 0, 0))

    def test_draynor_bank_spot_threats_match_live_death(self):
        entry = knowledge_data.npc_by_name("young_dark_wizard")[0]
        near = [spot for spot in entry.spawns if abs(spot.x - 3091) + abs(spot.z - 3243) < 30]
        self.assertTrue(near)
        npcs = [(spot.x, spot.z, spot.level, entry.huntrange, entry.vislevel, True) for spot in entry.spawns]
        self.assertTrue(spots.threats_at(3091, 3243, 0, npcs, 3))
        self.assertEqual(spots.threats_at(3091, 3243, 0, npcs, 15), [])


class RunTest(unittest.TestCase):
    def test_want_run_only_at_full_energy(self):
        self.assertTrue(bot_with_stats({}, energy=100).want_run())
        self.assertFalse(bot_with_stats({}, energy=99).want_run())

    def test_combat_level_from_world_stats(self):
        bot = bot_with_stats({0: (40, 0), 1: (40, 0), 2: (40, 0), 3: (40, 0), 4: (1, 0), 5: (1, 0), 6: (1, 0)})
        self.assertEqual(bot.combat_level(), 46)
        self.assertIsNone(bot_with_stats({8: (30, 1000)}).combat_level())


if __name__ == "__main__":
    unittest.main()
