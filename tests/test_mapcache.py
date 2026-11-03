import unittest


from orchestration.mapcache import MapCache


class MapCacheTest(unittest.TestCase):
    def test_floor_palette_is_loaded(self):
        cache = MapCache()
        self.assertGreater(len(cache._floor()), 100)

    def test_overlay_colour_uses_the_client_chain(self):
        cache = MapCache()
        # flo 48 muddygrass through the HSL table + gamma 0.8
        self.assertEqual(cache._flo_overlay_colour(48), 0x54600E)
        # water is a textured floor: the texture palette average, gamma 1.4
        self.assertEqual(cache._flo_overlay_colour(5), 0x576A92)

    def test_lumbridge_ground_contains_water(self):
        pixels = MapCache().render_ground(0, 3224, 3200, 24)
        self.assertIn(0x576A92, pixels)

    def test_missing_map_square_renders_black(self):
        self.assertEqual(set(MapCache().render_ground(0, 96, 96, 4)), {0})

    def test_underlay_is_blended_over_neighbours(self):
        cache = MapCache()
        # two tiles of the same flo id in different neighbourhoods differ,
        # because the client averages the HSL of the 10x10 surrounding tiles
        self.assertNotEqual(cache._blend(0, 3210, 3200), cache._blend(0, 3190, 3180))


if __name__ == "__main__":
    unittest.main()
