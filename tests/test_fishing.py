import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "gamedata"))

import gamedata as knowledge

from scripts.fishing import run


class FishTest(unittest.TestCase):
    def test_npc_matches_tier_by_name_not_id(self):
        tier = next(t for t in knowledge.all_fish_tiers() if t.category == "saltfish")
        types = [{"name": "hans"}] * 5 + [{"name": "0_99_99_saltfish"}]
        bot = SimpleNamespace(_loctypes={"npc": types})
        self.assertTrue(run.npc_matches_tier(bot, {"type": 5}, tier))
        self.assertFalse(run.npc_matches_tier(bot, {"type": 0}, tier))
        self.assertFalse(run.npc_matches_tier(bot, {"type": 999}, tier))

    def test_sell_batches_cover_amount(self):
        self.assertEqual(run.sell_batches(0), [])
        self.assertEqual(run.sell_batches(1), [(2, 1)])
        self.assertEqual(run.sell_batches(6), [(3, 5), (2, 1)])
        self.assertEqual(run.sell_batches(23), [(4, 10), (4, 10), (2, 1), (2, 1), (2, 1)])
        for amount in (1, 7, 13, 27):
            self.assertEqual(sum(count for _op, count in run.sell_batches(amount)), amount)

    def test_withdraw_batches_cover_amount(self):
        self.assertEqual(run.withdraw_batches(0), [])
        self.assertEqual(run.withdraw_batches(1), [(1, 1)])
        self.assertEqual(run.withdraw_batches(6), [(2, 5), (1, 1)])
        for amount in (1, 5, 13, 27):
            self.assertEqual(sum(count for _op, count in run.withdraw_batches(amount)), amount)

    def test_best_tier_gates_levels(self):
        self.assertEqual(knowledge.best_fish_tier(1).products[0], "raw_shrimp")
        self.assertEqual(knowledge.best_fish_tier(5).products[0], "raw_sardine")
        self.assertEqual(knowledge.best_fish_tier(35).products[0], "raw_tuna")

    def test_pick_fish_spot_near_start_at_level_1(self):
        tier, spawn, bank = run.pick_fish_spot(knowledge.all_fish_tiers(), 1, set(), 3, 3202, 3249)
        self.assertIsNotNone(tier)
        self.assertEqual(tier.products[0], "raw_shrimp")
        self.assertIsNotNone(spawn)
        self.assertIsNotNone(bank)
        self.assertFalse(bank.members)
        self.assertLessEqual(abs(spawn.x - bank.x) + abs(spawn.z - bank.z), 200)
        self.assertEqual(tier.category, "saltfish")

    def test_pick_fish_spot_prefers_higher_tier(self):
        tier, spawn, bank = run.pick_fish_spot(knowledge.all_fish_tiers(), 40, set(), 3, 3202, 3249)
        self.assertIsNotNone(tier)
        # From Lumbridge the reachable pair is pike (lobster spots are
        # bank-poor from here: nearest Catherby pair is 500+ away).
        self.assertEqual(tier.products[0], "raw_pike")

    def test_pick_fish_spot_lobster_near_catherby(self):
        tier, spawn, bank = run.pick_fish_spot(knowledge.all_fish_tiers(), 40, set(), 3, 2945, 3367)
        self.assertIsNotNone(tier)
        self.assertEqual(tier.products[0], "raw_lobster")
        self.assertFalse(bank.members)


if __name__ == "__main__":
    unittest.main()
