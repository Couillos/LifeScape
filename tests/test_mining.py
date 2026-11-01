import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "gamedata"))

import gamedata as knowledge
from gamedata.banks import Bank

from scripts.mining import run


class _StubWorld:
    x, z, level = 3222, 3222, 0
    locs = {}
    static_locs = {(3268, 3227, 0): (2882, 0, 0)}


class _StubBot:
    world = _StubWorld()
    _loctypes = {"loc": [None] * 2882 + [{"name": "border_gate_toll_left"}]}

    def combat_level(self):
        return 3

    def _ensure_nav(self):
        return None


class TrekTest(unittest.TestCase):
    def test_tile_walkable_open_and_blocked(self):
        import gamedata as knowledge  # noqa: F401  (ensures gamedata importable)
        from lifescape import collision as collision_flags
        from lifescape.collision import CollisionMap

        class _World:
            level = 0

        class _Bot:
            world = _World()
            _opened_map = None
            _collision = CollisionMap()

        bot = _Bot()
        bot._collision.set(10, 10, 0, 0)
        bot._collision.set(11, 11, 0, collision_flags.WALK_BLOCKED)
        self.assertTrue(run.tile_walkable(bot, 10, 10))
        self.assertFalse(run.tile_walkable(bot, 11, 11))

    def test_step_open_prefers_walkable(self):
        from lifescape import collision as collision_flags
        from lifescape.collision import CollisionMap

        class _World:
            level = 0
            x, z = 0, 0

        class _Bot:
            world = _World()
            _opened_map = None
            _collision = CollisionMap()

        bot = _Bot()
        for dx in range(-15, 16):
            bot._collision.set(dx, 0, 0, 0)
        bot._collision.set(12, 0, 0, collision_flags.WALK_BLOCKED)
        step_x, step_z = run.step_open(bot, 100, 0)
        self.assertTrue(run.tile_walkable(bot, step_x, step_z))
        self.assertLessEqual(abs(step_x - 12) + abs(step_z), 4)


class MiningTest(unittest.TestCase):
    def test_sell_batches_cover_amount(self):
        self.assertEqual(run.sell_batches(0), [])
        self.assertEqual(run.sell_batches(1), [(2, 1)])
        self.assertEqual(run.sell_batches(6), [(3, 5), (2, 1)])
        self.assertEqual(run.sell_batches(23), [(4, 10), (4, 10), (2, 1), (2, 1), (2, 1)])
        for amount in (1, 7, 13, 27):
            self.assertEqual(sum(count for _op, count in run.sell_batches(amount)), amount)

    def test_pick_tier_is_highest_affordable(self):
        tiers = run.usable_tiers()
        self.assertEqual(run.pick_tier(tiers, 1).product, "copper_ore")
        self.assertEqual(run.pick_tier(tiers, 15).product, "iron_ore")
        self.assertEqual(run.pick_tier(tiers, 30).product, "coal")

    def test_usable_tiers_are_f2p_with_products(self):
        for tier in run.usable_tiers():
            self.assertTrue(tier.product)
            self.assertFalse(tier.product_members)

    def test_nearest_spot_picks_closest(self):
        spots = knowledge.loc_spawns_near(("copperrock1", "copperrock2"), 3299, 3015, 0, radius=30)
        self.assertTrue(spots)
        best = run.nearest_spot(spots, 3299, 3015)
        self.assertTrue(all(abs(spot.x - 3299) + abs(spot.z - 3015) >= abs(best.x - 3299) + abs(best.z - 3015) for spot in spots))

    def test_pick_home_bank_has_ore_nearby(self):
        bank = run.pick_home_bank(_StubBot(), run.usable_tiers(), 1)
        self.assertIsNotNone(bank)
        self.assertFalse(bank.members)
        tier, spot, _use = run.pick_rock_spot(run.usable_tiers(), bank, 1, set(), 3)
        self.assertIsNotNone(tier)
        self.assertIsNotNone(spot)

    def test_pick_rock_spot_prefers_bank_proximity(self):
        bank = Bank(x=3091, z=3243, level=0, loc_id=2213, name="bankbooth", members=False)
        tier, spot, use_bank = run.pick_rock_spot(run.usable_tiers(), bank, 1, set(), 3)
        self.assertIsNotNone(tier)
        self.assertIn(tier.product, ("copper_ore", "tin_ore"))
        self.assertIsNotNone(spot)
        self.assertEqual((use_bank.x, use_bank.z), (3091, 3243))
        self.assertLessEqual(abs(spot.x - bank.x) + abs(spot.z - bank.z), 150)


if __name__ == "__main__":
    unittest.main()
