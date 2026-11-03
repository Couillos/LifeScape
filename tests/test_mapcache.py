import unittest


from orchestration.mapcache import MapCache


class MapCacheTest(unittest.TestCase):
    def test_floor_palette_is_loaded(self):
        cache = MapCache()
        self.assertGreater(len(cache._floor()), 100)

    def test_lumbridge_grid_has_grass_and_dirt(self):
        grid = MapCache().grid(0, 3222, 3218, 4)
        self.assertEqual(grid["size"], 9)
        self.assertEqual(len(grid["tiles"]), 9)
        self.assertEqual(len(grid["tiles"][0]), 9)
        colours = set(grid["palette"])
        self.assertTrue(any(colour != "#121212" for colour in colours))

    def test_missing_map_square_still_returns_a_grid(self):
        grid = MapCache().grid(0, 100, 100, 2)
        self.assertEqual(grid["size"], 5)
        self.assertIn("#121212", grid["palette"])

    def test_grid_carries_official_minimap_colours(self):
        grid = MapCache().grid(0, 3222, 3218)
        self.assertIn("#4f648d", grid["palette"])  # water, per floorcol.dat


if __name__ == "__main__":
    unittest.main()
