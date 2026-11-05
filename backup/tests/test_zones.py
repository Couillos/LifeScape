"""Zone labels parsed from the real worldmap jag, against known places."""

import os
import unittest

from lifescape.zones import TYPE_RADIUS, event_area, load_labels, nearest

HERE = os.path.dirname(os.path.abspath(__file__))
PACK_DIR = os.path.join(HERE, "..", "..", "lostcity", "engine", "data", "pack")


class ZonesTest(unittest.TestCase):

    @classmethod
    def setUpClass(cls):
        path = os.path.join(PACK_DIR, "mapview", "worldmap.jag")
        if not os.path.exists(path):
            raise unittest.SkipTest("worldmap.jag not built yet")

    def test_labels_load_and_cover_known_places(self):
        labels = load_labels(PACK_DIR)
        self.assertGreater(len(labels), 100)
        names = {name for name, _x, _z, _type in labels}
        self.assertIn("Varrock", names)
        self.assertIn("Lumbridge", names)

    def test_nearest_is_precise_at_varrock(self):
        labels = load_labels(PACK_DIR)
        found = nearest(labels, 3212, 3442)
        self.assertEqual(found[0], "Varrock")

    def test_nearest_prefers_town_over_far_region(self):
        # mine east of Varrock: no local label, the town claims it
        labels = load_labels(PACK_DIR)
        found = nearest(labels, 3285, 3365)
        self.assertEqual(found[0], "Varrock")

    def test_nearest_prefers_close_place_over_town(self):
        # Varrock palace courtyard: the courtyard wins over the town
        labels = load_labels(PACK_DIR)
        found = nearest(labels, 3213, 3483)
        self.assertEqual(found[0], "Palace")

    def test_nearest_region_catches_deep_wilderness(self):
        labels = load_labels(PACK_DIR)
        found = nearest(labels, 3100, 3700)
        self.assertEqual(found[0], "Wilderness")

    def test_event_area_names_instances(self):
        self.assertEqual(event_area(2911, 4575), "maze")
        self.assertEqual(event_area(2000, 4800 - 32), "mime")
        self.assertIsNone(event_area(3222, 3218))

    def test_zone_name_prefers_event_over_labels(self):
        from lifescape.client import Bot
        bot = Bot()
        bot.world.level, bot.world.x, bot.world.z = 0, 2911, 4575
        self.assertEqual(bot.zone_name(), ("Random event (maze)", None, None))
        bot.world.level, bot.world.x, bot.world.z = 0, 3222, 3218
        bot.paths = {"pack_dir": PACK_DIR, "types": None}
        name, dist, _type = bot.zone_name()
        self.assertEqual(name, "Lumbridge")
        self.assertIsNotNone(dist)

    def test_nearest_returns_none_far_away(self):
        labels = [("Somewhere", 3000, 3000, 0)]
        self.assertIsNone(nearest(labels, 0, 0))
        self.assertEqual(nearest(labels, 2990, 2990)[0], "Somewhere")


if __name__ == "__main__":
    unittest.main()
