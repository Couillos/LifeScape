"""build_legs() offline : backbone + banque->spot depuis gamedata."""

import unittest
from types import SimpleNamespace

from scripts.dev import check_routes


class CheckRoutesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import gamedata as knowledge
        cls.legs = check_routes.build_legs(knowledge)

    def test_backbone_present(self):
        names = [leg["name"] for leg in self.legs]
        for wanted in ("lumbridge-varrock", "varrock-falador",
                       "falador-port-sarim", "port-sarim-draynor"):
            self.assertIn(wanted, names)

    def test_legs_are_sane(self):
        self.assertTrue(self.legs)
        for leg in self.legs:
            self.assertIn(leg["expect"], ("reach", "diagnose"))
            (slevel, sx, sz), (dlevel, dx, dz) = leg["start"], leg["dest"]
            self.assertEqual(slevel, 0)
            self.assertEqual(dlevel, 0)
            self.assertNotEqual((sx, sz), (dx, dz))

    def test_skill_and_booth_legs_found(self):
        names = [leg["name"] for leg in self.legs]
        self.assertTrue(any(name.startswith("bank-tree-") for name in names))
        self.assertTrue(any(name.startswith("bank-rock-") for name in names))
        self.assertTrue(any(name.startswith("bank-booth-") for name in names))


class RestageLoopTest(unittest.TestCase):
    def make_bot(self, walks):
        from lifescape import collision as collision_mod
        from lifescape.client.core import Unreachable

        world = SimpleNamespace(x=0, z=0, level=0, ticks=0, position=(0, 0, 0))
        bot = SimpleNamespace(world=world, _tele_target=None, walks=list(walks))

        class FakeMap:
            def get(self, x, z, level):
                return collision_mod.OPEN

        bot.collision = FakeMap()

        def cheat(body):
            parts = body.split(" ")[1].split(",")
            level, zx, zz, lx, lz = (int(part) for part in parts)
            bot._tele_target = (level, (zx << 6) + lx, (zz << 6) + lz)

        def pump(n=1):
            world.ticks += n
            if bot._tele_target is not None:
                level, x, z = bot._tele_target
                world.level, world.x, world.z = level, x, z
                world.position = (level, x, z)
                bot._tele_target = None

        def walk_to(x, z, level=0, timeout=0, paid=None, run=None):
            effect = bot.walks.pop(0)
            if isinstance(effect, Exception):
                world.x, world.z = (500, 500)  # yank en cours de marche
                world.position = (world.level, 500, 500)
                raise effect
            world.x, world.z = x, z
            world.position = (world.level, x, z)
            return True

        bot.cheat = cheat
        bot.pump = pump
        bot.walk_to = walk_to
        bot.find_npc = lambda name: None
        return bot

    def test_restage_then_ok(self):
        from lifescape.client.core import Unreachable
        from scripts.dev import check_routes
        bot = self.make_bot([Unreachable("yank"), True])
        leg = {"name": "trek", "start": (0, 1, 1), "dest": (0, 60, 60), "expect": "reach"}
        outcome = check_routes.test_leg(bot, leg)
        self.assertTrue(outcome.startswith("restaged x1: ok"), outcome)

    def test_three_yanks_is_yanked(self):
        from lifescape.client.core import Unreachable
        from scripts.dev import check_routes
        bot = self.make_bot([Unreachable("y"), Unreachable("y"), Unreachable("y")])
        leg = {"name": "trek", "start": (0, 1, 1), "dest": (0, 60, 60), "expect": "reach"}
        self.assertEqual(check_routes.test_leg(bot, leg),
                         "unreachable: yanked by random event mid-leg")


if __name__ == "__main__":
    unittest.main()
