import unittest
from types import SimpleNamespace

from lifescape import collision as flags
from lifescape.client import Bot


class FakeMap:
    def __init__(self, blocked=()):
        self.blocked = set(blocked)

    def get(self, x, z, level):
        return flags.WALK_BLOCKED if (x, z) in self.blocked else flags.OPEN


SPOT = {"type": 0, "x": 3103, "z": 3092, "level": 0}


class FakeBot:
    collision = FakeMap()
    loctypes = {"npc": [{"name": "spot", "size": 1}]}
    _npc_size = Bot._npc_size
    _npc_clickable = Bot._npc_clickable
    approach_npc = Bot.approach_npc

    def __init__(self, x, z, npc=None):
        self.world = SimpleNamespace(x=x, z=z, level=0, ticks=0, npcs={1302: dict(npc or SPOT)})
        self.walks = []

    def ensure_nav(self):
        pass

    def dbg(self, *_args):
        pass

    def pump(self, ticks):
        self.world.ticks += ticks

    def walk_to(self, x, z, level=0, timeout=0, open_doors=True):
        self.walks.append((x, z))
        self.world.x, self.world.z = x, z


class NpcClickableTest(unittest.TestCase):
    def test_diagonal_tile_is_not_clickable(self):
        bot = FakeBot(3104, 3091)
        self.assertFalse(bot._npc_clickable(bot.world.npcs[1302]))

    def test_side_tiles_are_clickable(self):
        for x, z in ((3104, 3092), (3102, 3092), (3103, 3091), (3103, 3093)):
            bot = FakeBot(x, z)
            self.assertTrue(bot._npc_clickable(bot.world.npcs[1302]), (x, z))

    def test_other_level_is_not_clickable(self):
        bot = FakeBot(3104, 3092, dict(SPOT, level=1))
        self.assertFalse(bot._npc_clickable(bot.world.npcs[1302]))

    def test_approach_npc_leaves_a_diagonal_tile(self):
        bot = FakeBot(3104, 3091)
        self.assertTrue(bot.approach_npc(1302, timeout=100))
        self.assertEqual(abs(bot.world.x - SPOT["x"]) + abs(bot.world.z - SPOT["z"]), 1)

    def test_approach_npc_stays_when_already_side_by_side(self):
        bot = FakeBot(3104, 3092)
        self.assertTrue(bot.approach_npc(1302, timeout=100))
        self.assertEqual(bot.walks, [])


if __name__ == "__main__":
    unittest.main()
