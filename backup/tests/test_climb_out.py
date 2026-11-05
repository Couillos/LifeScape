"""climb_out fiable : table des transitions, sans boucle, sans un seul essai."""

import unittest
from types import SimpleNamespace

from scripts import randomevents


def make_bot(level=1, rows=(), ride=None):
    world = SimpleNamespace(level=level, x=100, z=100, ticks=0,
                            position=(level, 100, 100))

    def pump(n=1):
        world.ticks += n

    bot = SimpleNamespace(world=world, _portal_rows=list(rows),
                          _ride_portal=ride or (lambda row: False),
                          pump=pump, intend=lambda *a: None,
                          position=(level, 100, 100))
    return bot


def row(name, frm, to, op=1):
    return {"name": name, "from": list(frm), "to": list(to),
            "climb_loc": 1, "climb_op": op, "footprint": [[frm[1], frm[2]]]}


class ClimbOutTest(unittest.TestCase):
    def test_ground_returns_at_once(self):
        calls = []
        bot = make_bot(level=0, ride=lambda row: calls.append(row) or True)
        self.assertTrue(randomevents.climb_out(bot))
        self.assertEqual(calls, [])

    def test_nearest_down_first_then_next(self):
        order = []

        def ride(r):
            order.append(r["name"])
            if r["name"] == "far":
                bot.world.level = 0
                return True
            return False

        bot = make_bot(level=1, rows=[
            row("far", (1, 200, 200), (0, 200, 200)),
            row("near", (1, 105, 105), (0, 105, 105)),
        ], ride=ride)
        self.assertTrue(randomevents.climb_out(bot))
        self.assertEqual(order, ["near", "far"])

    def test_no_way_down_is_false(self):
        from unittest import mock
        from lifescape.client.core import Unreachable
        bot = make_bot(level=2, rows=[])
        with mock.patch.object(randomevents.tutorial_common, "find_loc",
                               side_effect=Unreachable("nope")):
            self.assertFalse(randomevents.climb_out(bot))

    def test_steps_off_fail_spot_on_ground(self):
        from lifescape import collision as collision_mod
        bot = make_bot(level=0, rows=[])
        bot.world.x, bot.world.z = 3283, 3218  # FAIL_TELEPORTS
        assert (0, 3283, 3218) in randomevents.FAIL_TELEPORTS
        walkable = {(3284, 3218, 0): collision_mod.OPEN}

        class Map:
            def get(self, x, z, level):
                return walkable.get((x, z, level), collision_mod.WALK_BLOCKED)

        bot.collision = Map()
        stepped = []

        def step_on(x, z, level, deadline):
            stepped.append((x, z))
            bot.world.x, bot.world.z = x, z
            return True

        bot.try_step_on = step_on
        self.assertTrue(randomevents.climb_out(bot))
        self.assertEqual(stepped, [(3284, 3218)])

    def test_stuck_on_fail_spot_is_false(self):
        from lifescape import collision as collision_mod
        bot = make_bot(level=0, rows=[])
        bot.world.x, bot.world.z = 3283, 3218
        assert (0, 3283, 3218) in randomevents.FAIL_TELEPORTS

        class Map:
            def get(self, x, z, level):
                return collision_mod.WALK_BLOCKED

        bot.collision = Map()
        bot.try_step_on = lambda *a: False
        self.assertFalse(randomevents.climb_out(bot))


if __name__ == "__main__":
    unittest.main()
