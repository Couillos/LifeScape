import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape import waits


def fake_bot():
    bot = SimpleNamespace()
    bot.world = SimpleNamespace(ticks=0, stats={3: (10, 10)})

    def pump(n=1):
        bot.world.ticks += n

    bot.pump = pump
    return bot


class WaitsTest(unittest.TestCase):
    def test_wait_ticks_advances_to_target(self):
        bot = fake_bot()
        waits.wait_ticks(bot, 10)
        self.assertGreaterEqual(bot.world.ticks, 10)
        self.assertLess(bot.world.ticks, 20)

    def test_wait_hitpoints_returns_when_healed(self):
        bot = fake_bot()
        bot.world.stats = {3: (9, 10)}
        seen = []

        def pump(n=5):
            bot.world.ticks += 1
            seen.append(bot.world.ticks)
            if bot.world.ticks >= 5:
                bot.world.stats = {3: (10, 10)}

        bot.pump = pump
        self.assertTrue(waits.wait_hitpoints(bot, 10, timeout=200))
        self.assertLess(len(seen), 200)

    def test_wait_hitpoints_times_out(self):
        bot = fake_bot()
        bot.world.stats = {3: (5, 10)}
        self.assertFalse(waits.wait_hitpoints(bot, 10, timeout=7))
        self.assertGreaterEqual(bot.world.ticks, 7)

    def test_ban_area_covers_radius(self):
        bans = {}
        waits.ban_area(bans, 100, 200, 2, 999)
        self.assertEqual(len(bans), 25)
        self.assertIn((100, 200), bans)
        self.assertIn((102, 202), bans)
        self.assertNotIn((103, 200), bans)
