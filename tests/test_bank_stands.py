"""Stands de banque : un par banque F2P, marchable et a portee du booth."""

import os
import unittest

from lifescape import banks as banks_mod
from lifescape import collision as collision_mod
from lifescape import f2p as f2p_mod
from lifescape import reach as reach_mod

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
PACK = os.path.normpath(os.path.join(ROOT, "..", "lostcity", "engine", "data", "pack"))


class BankStandsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.loctypes = collision_mod.load_types(os.path.join(ROOT, "lifescape", "data", "gametypes.json"))
        cls.collision, cls.static_locs, _, _ = collision_mod.load_maps(PACK, cls.loctypes)
        cls.freemap = f2p_mod.load_freemap(f2p_mod.csv_path(PACK))

    def test_every_f2p_bank_has_a_stand(self):
        import gamedata as knowledge
        banks = [b for b in knowledge.all_banks() if not b.members and b.level == 0]
        self.assertTrue(banks)
        for bank in banks:
            with self.subTest(bank=(bank.x, bank.z)):
                stand = banks_mod.stand_for(bank.x, bank.z, bank.level)
                self.assertIsNotNone(stand)

    def test_stands_are_walkable_f2p_and_in_reach(self):
        import gamedata as knowledge
        for entry in banks_mod.table():
            stand = entry["stand"]
            sx, sy, slevel = stand
            with self.subTest(stand=tuple(stand)):
                self.assertTrue(collision_mod.tile_walkable(self.collision.get(sx, sy, slevel)))
                self.assertTrue(f2p_mod.is_f2p(self.freemap, sx, sy))
                reached_one = False
                for booth_x, booth_z in entry["booths"]:
                    loc = self.static_locs.get((booth_x, booth_z, slevel))
                    if loc is None:
                        continue
                    loc_entry = self.loctypes["loc"][loc[0]]
                    width = int(loc_entry.get("width", 1) or 1)
                    height = int(loc_entry.get("length", 1) or 1)
                    if reach_mod.reached(self.collision.get, slevel, sx, sy, booth_x, booth_z,
                                         width, height, loc[2], loc[1], 0):
                        reached_one = True
                        break
                self.assertTrue(reached_one, entry["booths"])


if __name__ == "__main__":
    unittest.main()
