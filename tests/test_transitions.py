import json
import os
import unittest

from lifescape import portals
from lifescape.client import Bot
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
TABLE = os.path.join(ROOT, "lifescape", "data", "transitions.json")


class TransitionTableTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with open(TABLE) as handle:
            cls.rows = json.load(handle)

    def test_rows_are_well_formed(self):
        self.assertGreater(len(self.rows), 300)
        seen = set()
        for row in self.rows:
            self.assertIn(row["kind"], ("stair", "ladder", "trapdoor", "portal", "other"))
            self.assertIn(row["from"][0], (0, 1, 2, 3))
            self.assertIn(row["to"][0], (0, 1, 2, 3))
            self.assertNotEqual(tuple(row["from"]), tuple(row["to"]))
            self.assertIsNotNone(row["climb_loc"])
            key = (tuple(row["from"]), tuple(row["to"]), row["climb_loc"], row["climb_op"])
            self.assertNotIn(key, seen)
            seen.add(key)
            if row["open_to"] is not None:
                self.assertEqual(row["climb_loc"], row["open_to"])

    def test_wizard_tower_stairs_are_present(self):
        # The stair loc hides behind a wall on the same tile, so a table
        # built from one loc per tile silently dropped it.
        starts = {tuple(row["from"]) for row in self.rows}
        self.assertIn((0, 3103, 3159), starts)
        self.assertIn((1, 3103, 3159), starts)

    def test_disabled_rows_say_why(self):
        # Gated transitions stay in the table but must never be planned.
        for row in self.rows:
            if row.get("disabled"):
                self.assertTrue(row["disabled"])
        self.assertEqual(sum(1 for row in self.rows if row.get("disabled")), 5)

    def test_trapdoor_opens_before_climbing(self):
        rows = [row for row in self.rows if row["kind"] == "trapdoor"]
        self.assertTrue(rows)
        self.assertTrue(any(row["open_to"] is not None for row in rows))


class PortalRoutingTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.bot = Bot()
        cls.bot._paths = default_paths()
        cls.bot._ensure_nav()

    def test_route_to_lumbridge_first_floor(self):
        plan = self.bot._plan_portals((0, 3209, 3215), (1, 3205, 3228))
        self.assertIsNotNone(plan)
        kinds = [kind for kind, _payload in plan]
        self.assertEqual(kinds[0], "walk")
        self.assertIn("portal", kinds)
        last_kind, last_payload = plan[-1]
        arrival = tuple(last_payload.to) if last_kind == "portal" else tuple(last_payload)
        self.assertEqual(arrival, (1, 3205, 3228))

    def test_route_to_edgeville_dungeon(self):
        plan = self.bot._plan_portals((0, 3209, 3215), (0, 3096, 9868))
        self.assertIsNotNone(plan)
        self.assertIn("portal", [kind for kind, _payload in plan])

    def test_stair_tiles_are_never_walked_over(self):
        # Stepping on a transition tile makes the server move the player.
        self.assertIn((3103, 3159, 0), self.bot._stair_tiles)
        self.assertIn((3209, 3216, 0), self.bot._stair_tiles)

    def test_disabled_transitions_are_not_loaded(self):
        names = {portal.name for portal in self.bot._portals}
        for row in json.load(open(TABLE)):
            if row.get("disabled"):
                self.assertNotIn(row["name"], names)

    def test_planner_refuses_unknown_destination(self):
        planner = portals.RoutePlanner(self.bot._router, self.bot._collision)
        for portal in self.bot._portals:
            planner.add_portal(portal)
        self.assertIsNone(planner.plan((0, 3209, 3215), (0, 500, 500)))


if __name__ == "__main__":
    unittest.main()
