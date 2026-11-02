import json
import os
import unittest
from types import SimpleNamespace


import gamedata as knowledge
from gamedata.banks import Bank

from scripts.woodcut import run


def stub_bot(x, z, level=0):
    with open(os.path.join(os.path.dirname(__file__), "..", "lifescape", "data", "gametypes.json")) as handle:
        loctypes = json.load(handle)
    world = SimpleNamespace(x=x, z=z, level=level, locs={}, static_locs={(3268, 3227, 0): (2882, 0, 0)})
    bot = SimpleNamespace(world=world, _loctypes=loctypes)
    bot._ensure_nav = lambda: None
    return bot


class WoodcutTest(unittest.TestCase):
    def test_sell_batches_cover_amount(self):
        self.assertEqual(run.sell_batches(0), [])
        self.assertEqual(run.sell_batches(1), [(2, 1)])
        self.assertEqual(run.sell_batches(6), [(3, 5), (2, 1)])
        self.assertEqual(run.sell_batches(23), [(4, 10), (4, 10), (2, 1), (2, 1), (2, 1)])
        for amount in (1, 7, 13, 27):
            self.assertEqual(sum(count for _op, count in run.sell_batches(amount)), amount)

    def test_pick_tier_is_highest_affordable(self):
        tiers = knowledge.all_tiers()
        self.assertEqual(run.pick_tier(tiers, 1).product, "logs")
        self.assertEqual(run.pick_tier(tiers, 15).product, "oak_logs")
        self.assertEqual(run.pick_tier(tiers, 30).product, "willow_logs")
        self.assertEqual(run.pick_tier(tiers, 0).product, "logs")

    def test_nearest_spot_picks_closest(self):
        spots = knowledge.loc_spawns_near(("oaktree",), 3192, 3213, 0, radius=30)
        self.assertTrue(spots)
        best = run.nearest_spot(spots, 3192, 3213)
        self.assertTrue(all(abs(spot.x - 3192) + abs(spot.z - 3213) >= abs(best.x - 3192) + abs(best.z - 3213) for spot in spots))

    def test_pick_tree_spot_prefers_bank_proximity(self):
        bank = Bank(x=3091, z=3243, level=0, loc_id=2213, name="bankbooth", members=False)
        tier, spot, use_bank = run.pick_tree_spot(knowledge.all_tiers(), bank, 1, set(), 3)
        self.assertIsNotNone(tier)
        self.assertEqual(tier.product, "logs")
        self.assertIsNotNone(spot)
        self.assertEqual((use_bank.x, use_bank.z), (3091, 3243))
        self.assertLessEqual(abs(spot.x - bank.x) + abs(spot.z - bank.z), 150)


    def test_bank_is_nearest_same_side_not_pinned(self):
        # No pinned village: the bank follows the player, same side of
        # the toll gate when possible, never members, same level.
        lumbridge = run.nearest_free_bank(stub_bot(3222, 3222))
        self.assertIsNotNone(lumbridge)
        self.assertLess(lumbridge.x, 3268)
        self.assertFalse(lumbridge.members)
        self.assertEqual(lumbridge.level, 0)
        alkharid = run.nearest_free_bank(stub_bot(3300, 3180))
        self.assertIsNotNone(alkharid)
        self.assertGreaterEqual(alkharid.x, 3268)
        varrock = run.nearest_free_bank(stub_bot(3210, 3420))
        self.assertIsNotNone(varrock)
        self.assertLess(varrock.x, 3268)
        self.assertLess(abs(varrock.x - 3210) + abs(varrock.z - 3420), 150)


    def test_ban_area_covers_radius(self):
        bans = {}
        run.ban_area(bans, 100, 200, 2, 999)
        self.assertEqual(len(bans), 25)
        self.assertIn((100, 200), bans)
        self.assertIn((102, 202), bans)
        self.assertIn((98, 198), bans)
        self.assertNotIn((103, 200), bans)
        self.assertNotIn((100, 203), bans)

    def test_is_stump_needs_server_confirmation(self):
        stump_bot = stub_bot(0, 0)
        stump_bot.world.locs = {(5, 5, 0): (1342, 0, 0)}
        self.assertTrue(run.is_stump(stump_bot, 5, 5))
        booth_bot = stub_bot(0, 0)
        booth_bot.world.locs = {(5, 5, 0): (2213, 0, 0)}
        self.assertFalse(run.is_stump(booth_bot, 5, 5))
        blind_bot = stub_bot(0, 0)
        blind_bot.world.locs = {}
        self.assertFalse(run.is_stump(blind_bot, 5, 5))


    def test_wait_ticks_uses_server_ticks(self):
        world = SimpleNamespace(ticks=0)

        class FakeBot:
            pass

        bot = FakeBot()
        bot.world = world

        def pump(n=1):
            world.ticks += 3

        bot.pump = pump
        run.wait_ticks(bot, 10)
        self.assertGreaterEqual(world.ticks, 10)
        self.assertLess(world.ticks, 20)


if __name__ == "__main__":
    unittest.main()
