import unittest


import gamedata as knowledge

from lifescape import spots as spots_module
from scripts import hazards


class FakeWorld:
    def __init__(self, x, z, npcs=None, hitpoints=10):
        self.x = x
        self.z = z
        self.level = 0
        self.npcs = npcs or {}
        self.stats = {3: (hitpoints, 0)}
        self.ticks = 0


class FakeBot:
    def __init__(self, world, moves=0):
        self.world = world
        self.moves = moves
        self.walked = []

    @property
    def position(self):
        return (self.world.level, self.world.x, self.world.z)

    def intend(self, _text):
        pass

    def combat_level(self):
        return 3

    def walk_to(self, x, z, level=0, run=None, timeout=180, open_doors=False):
        self.walked.append((x, z))
        self.world.x = x
        self.world.z = z
        return True

    def pump(self, count=1):
        self.world.ticks += count
        if self.moves > 0:
            self.moves -= 1
            if self.moves == 0:
                self.world.stats[3] = (10, 0)


def hunter_at(x, z):
    npc_id = next(entry.npc_id for entry in knowledge.all_npcs() if entry is not None and entry.hunts_players and entry.huntrange == 3)
    return {1: {"type": npc_id, "x": x, "z": z, "level": 0}}


class HazardsTest(unittest.TestCase):
    def test_avoid_steps_outside_danger_radius(self):
        bot = FakeBot(FakeWorld(10, 10, hunter_at(14, 10)))
        npc = hazards.nearest_threat(bot)
        radius = npc[3] + hazards.LIVE_MARGIN
        self.assertTrue(hazards.avoid(bot))
        self.assertGreater(spots_module.chebyshev(bot.world.x, bot.world.z, 14, 10), radius)

    def test_avoid_ignores_dead_or_distant_hunters(self):
        bot = FakeBot(FakeWorld(10, 10, {}))
        self.assertFalse(hazards.avoid(bot))
        self.assertEqual(bot.walked, [])

    def test_wait_respawn_returns_when_hitpoints_come_back(self):
        bot = FakeBot(FakeWorld(3222, 3222, hitpoints=0), moves=2)
        self.assertTrue(hazards.wait_respawn(bot))

    def test_hitpoints_reads_stats(self):
        self.assertEqual(hazards.hitpoints(FakeBot(FakeWorld(0, 0, hitpoints=7))), 7)
