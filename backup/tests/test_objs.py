import json
import os
import unittest

from lifescape import objs
from lifescape import portals
from lifescape import route as route_mod
from lifescape.collision import CollisionMap


def open_map(width, height):
    world = CollisionMap()
    for x in range(width):
        for z in range(height):
            world.set(x, z, 0, 0)
    return world


class ObjsTest(unittest.TestCase):
    def test_take_and_drop_ops(self):
        types = json.load(open(os.path.join(os.path.dirname(__file__), "..", "lifescape", "data", "gametypes.json")))
        logs = next(i for i, o in enumerate(types["obj"]) if o and o["name"] == "logs")
        self.assertEqual(objs.take_op(types["obj"], logs), 3)
        self.assertEqual(objs.drop_op(types["obj"], logs), 5)


class PortalsTest(unittest.TestCase):
    def test_chains_walk_and_portal(self):
        world = open_map(140, 10)
        router = route_mod.Router()
        planner = portals.RoutePlanner(router, world)
        planner.add_portal(portals.Portal("stairs", (0, 120, 0), (0, 130, 0), None))
        legs = planner.plan((0, 0, 0), (0, 139, 0))
        kinds = [leg[0] for leg in legs]
        self.assertIn("portal", kinds)
        self.assertIn("walk", kinds)
        walk_to_portal = [leg for leg in legs if leg[0] == "walk"][0][1]
        self.assertEqual(walk_to_portal[1:], (120, 0))

    def test_no_route_returns_none(self):
        world = open_map(10, 10)
        router = route_mod.Router()
        planner = portals.RoutePlanner(router, world)
        self.assertIsNone(planner.plan((0, 0, 0), (0, 60, 60)))


if __name__ == "__main__":
    unittest.main()
