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

    def test_grid_carries_floor_ids_for_detailed_mode(self):
        grid = MapCache().grid(0, 3222, 3218, 4)
        self.assertEqual(len(grid["flo"]), 9)
        self.assertEqual(len(grid["flo"][0]), 9)
        self.assertTrue(any(floor_id >= 0 for row in grid["flo"] for floor_id in row))

    def test_flo_info_maps_textures(self):
        info = MapCache().flo_info()
        self.assertGreater(len(info), 100)
        self.assertEqual(info["5"]["texture"], "water")
        self.assertIsNone(info["48"]["texture"])

    def test_texture_pixels_decode_and_reuse_ram(self):
        cache = MapCache()
        width, height, pixels = cache._texture_pixels("water")
        self.assertEqual((width, height), (128, 128))
        self.assertEqual(len(pixels), 128 * 128 * 3)
        self.assertIs(pixels, cache._texture_pixels("water")[2])

    def test_detail_table_downsamples_on_server(self):
        cache = MapCache()
        table = cache.detail_table(8)
        self.assertEqual(table["y"], 8)
        self.assertEqual(len(table["tex"]["water"]), 64)
        self.assertEqual(len(cache.detail_table(1)["tex"]["water"]), 1)
        self.assertEqual(cache.detail_table(0)["y"], 1)
        self.assertEqual(cache.detail_table(99)["y"], 32)
        self.assertIs(table["tex"]["water"], cache.detail_table(8)["tex"]["water"])


if __name__ == "__main__":
    unittest.main()
