"""walk_checked s'arrete a cote d'une cible bloque (pas de boucle)."""

import unittest
from types import SimpleNamespace
from unittest import mock

from lifescape import collision as collision_mod


def make_bot(blocked=frozenset()):
    world = SimpleNamespace(x=6, z=5, level=0, ticks=0, position=(0, 6, 5))
    calls = []

    class FakeMap:
        def get(self, x, z, level):
            if (x, z) in blocked:
                return collision_mod.WALK_BLOCKED
            return collision_mod.OPEN

    bot = SimpleNamespace(world=world, collision=FakeMap(), walks=calls)
    bot.opened_map = None

    def walk_to(x, z, level=0, **kwargs):
        calls.append((x, z))
        if len(calls) > 50:
            raise TimeoutError("tight loop: walk_to called %d times" % len(calls))
        return True

    bot.walk_to = walk_to
    return bot, calls


class WalkCheckedTest(unittest.TestCase):
    def test_blocked_adjacent_returns_at_once(self):
        from scripts.woodcut import walk as woodcut_walk
        bot, calls = make_bot(blocked=frozenset({(5, 5)}))
        with mock.patch.object(woodcut_walk, "hitpoints", lambda bot: 99), \
             mock.patch.object(woodcut_walk, "randomevents") as events:
            events.greet.return_value = True
            self.assertTrue(woodcut_walk.walk_checked(bot, 5, 5, timeout=15000))
        self.assertLessEqual(len(calls), 2)

    def test_blocked_adjacent_fishing(self):
        from scripts.fishing import walk as fishing_walk
        bot, calls = make_bot(blocked=frozenset({(5, 5)}))
        with mock.patch.object(fishing_walk, "hitpoints", lambda bot: 99), \
             mock.patch.object(fishing_walk, "randomevents") as events:
            events.greet.return_value = True
            self.assertTrue(fishing_walk.walk_checked(bot, 5, 5, timeout=15000))
        self.assertLessEqual(len(calls), 2)

    def test_blocked_adjacent_mining(self):
        from scripts.mining import walk as mining_walk
        bot, calls = make_bot(blocked=frozenset({(5, 5)}))
        with mock.patch.object(mining_walk, "hitpoints", lambda bot: 99), \
             mock.patch.object(mining_walk, "randomevents") as events:
            events.greet.return_value = True
            self.assertTrue(mining_walk.walk_checked(bot, 5, 5, timeout=15000))
        self.assertLessEqual(len(calls), 2)


if __name__ == "__main__":
    unittest.main()
