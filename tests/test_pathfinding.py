import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape import collision as flags
from lifescape.client import Bot
from lifescape.route import Router


class FakeMap:
    def __init__(self, blocked=()):
        self.blocked = set(blocked)

    def get(self, x, z, level):
        return flags.WALK_BLOCKED if (x, z) in self.blocked else flags.OPEN


class PathfindingTest(unittest.TestCase):
    def test_reachable_target_is_exact(self):
        router = Router()
        path = router.find_path(FakeMap(), 0, 0, 0, 4, 0)
        self.assertEqual(path[-1], (0, 4, 0))

    def test_blocked_target_returns_path_distance_nearest(self):
        wall = {(5, z) for z in range(-4, 5)}
        router = Router()
        path = router.find_path(FakeMap(wall), 0, 0, 0, 5, 0)
        self.assertTrue(path)
        self.assertEqual(path[-1], (0, 4, 0))

    def test_report_names_the_blocker_and_nearest_tile(self):
        wall = {(5, z) for z in range(-4, 5)}
        router = Router()

        def name_at(x, z, level):
            return "closed door" if (x, z) in wall else None

        report = router.unreachable_report(FakeMap(wall), 0, 0, 0, 5, 0, name_at)
        self.assertTrue(report["dest_blocked"])
        self.assertEqual(report["nearest"], (4, 0))
        self.assertEqual(report["spread"], 1)
        self.assertTrue(any(name == "closed door" for _x, _z, name in report["blockers"]))

    def test_explain_unreachable_mentions_cause(self):
        wall = {(5, z) for z in range(-4, 5)}

        class FakeBot:
            _router = Router()
            _opened_map = FakeMap(wall)
            _collision = FakeMap(wall)
            world = SimpleNamespace(
                level=0,
                x=0,
                z=0,
                loc_at=lambda x, z, level: (0, 0, 0) if (x, z) in wall else None,
            )
            _loctypes = {"loc": [{"name": "closed door"}]}
            _loc_name_at = Bot._loc_name_at
            _unreachable_hints = Bot._unreachable_hints
            _explain_unreachable = Bot._explain_unreachable

        fake = FakeBot()
        message = Bot._explain_unreachable(fake, 5, 0, False)
        self.assertIn("no path to 5,0", message)
        self.assertIn("closed door", message)
        self.assertIn("nearest reachable by path 4,0", message)
        self.assertIn("open the gate/door", message)


if __name__ == "__main__":
    unittest.main()
