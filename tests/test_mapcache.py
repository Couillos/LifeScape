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

    def test_grid_uses_terrain_colours(self):
        grid = MapCache().grid(0, 3222, 3218)
        # water = texture average with gamma 1.4, like the client minimap
        self.assertIn("#5671ac", grid["palette"])

    def test_palette_uses_the_client_chain(self):
        palette = MapCache().palette()
        # flo 48 muddygrass through the HSL table + gamma, like Pix3D.colourTable
        self.assertEqual(palette[48], "#404c07")
        # water uses its texture average with gamma 1.4 (textured floor)
        self.assertEqual(palette[5], "#5671ac")

    def test_underlay_is_blended_over_neighbours(self):
        cache = MapCache()
        # two tiles of the same flo id in different neighbourhoods differ,
        # because the client averages the HSL of the 11x11 surrounding tiles
        first = cache.colour_at(0, 3210, 3200)
        second = cache.colour_at(0, 3190, 3180)
        self.assertNotEqual(first, second)


if __name__ == "__main__":
    unittest.main()
