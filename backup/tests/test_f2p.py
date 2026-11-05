"""F2P en dur : la carte free2play du serveur borne le routage."""

import os
import unittest

from lifescape import collision as collision_mod
from lifescape import f2p as f2p_mod
from lifescape import nav as nav_mod
from lifescape.connection import default_paths


def pack_dir():
    return default_paths()["pack_dir"]


class F2PTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.freemap = f2p_mod.load_freemap(f2p_mod.csv_path(pack_dir()))

    def test_lumbridge_is_f2p(self):
        self.assertTrue(f2p_mod.is_f2p(self.freemap, 3222, 3218))

    def test_taverley_gate_is_members(self):
        self.assertFalse(f2p_mod.is_f2p(self.freemap, 2935, 3450))

    def test_view_blocks_members_delegates_f2p(self):
        class Base:
            def get(self, x, z, level):
                return collision_mod.OPEN

            def has(self, x, z, level):
                return True

        view = f2p_mod.F2PMap(Base(), self.freemap)
        self.assertEqual(view.get(2935, 3450, 0), collision_mod.WALK_BLOCKED)
        self.assertEqual(view.get(3222, 3218, 0), collision_mod.OPEN)

    def test_door_map_never_opens_members(self):
        class Base:
            def get(self, x, z, level):
                return collision_mod.OPEN

        doors = {(2935, 3450, 0)}
        view = nav_mod.DoorMap(Base(), doors, self.freemap)
        self.assertEqual(view.get(2935, 3450, 0), collision_mod.WALK_BLOCKED)
        view = nav_mod.DoorMap(Base(), {(3222, 3218, 0)}, self.freemap)
        self.assertEqual(view.get(3222, 3218, 0), collision_mod.OPEN)


if __name__ == "__main__":
    unittest.main()
