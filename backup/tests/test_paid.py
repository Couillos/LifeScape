"""Option paid : les peages sont barres par defaut, ouverts si paye."""

import unittest
from types import SimpleNamespace

from lifescape.client.movement.base import MovementBase


def fake_bot(**attrs):
    class FakeBot(MovementBase):
        pass
    bot = FakeBot()
    base = {"_blocked_doors": set(), "toll_doors": set(), "allow_paid_paths": False}
    base.update(attrs)
    for key, value in base.items():
        setattr(bot, key, value)
    return bot


class PaidTest(unittest.TestCase):
    def setUp(self):
        self.spot = (3267, 3272, 0, 0, 0, False, 1, 1, 1, 42)

    def test_toll_locked_by_default(self):
        bot = fake_bot(toll_doors={id(self.spot)})
        self.assertTrue(MovementBase._toll_locked(bot, self.spot))
        self.assertTrue(MovementBase._door_avoid(bot, self.spot))
        self.assertEqual(MovementBase._toll_blocked(bot), {id(self.spot)})

    def test_toll_open_when_paid(self):
        bot = fake_bot(toll_doors={id(self.spot)}, allow_paid_paths=True)
        self.assertFalse(MovementBase._toll_locked(bot, self.spot))
        self.assertFalse(MovementBase._door_avoid(bot, self.spot))
        self.assertEqual(MovementBase._toll_blocked(bot), set())

    def test_normal_door_unaffected(self):
        bot = fake_bot(toll_doors={999})
        self.assertFalse(MovementBase._toll_locked(bot, self.spot))
        self.assertFalse(MovementBase._door_avoid(bot, self.spot))

    def test_blocked_stays_blocked_when_paid(self):
        bot = fake_bot(_blocked_doors={id(self.spot)}, allow_paid_paths=True)
        self.assertTrue(MovementBase._door_avoid(bot, self.spot))


class TollDialogTest(unittest.TestCase):
    def make_bot(self):
        from lifescape.client.movement.doors import MovementDoors

        class TollBot(MovementDoors):
            pass

        bot = TollBot()
        bot.world = SimpleNamespace(x=3260, z=3227, level=0, modal=None,
                                    ticks=0, locs={}, messages=[], texts={})
        bot.world.loc_at = lambda x, z, level: None
        bot.loctypes = {"loc": [{"ops": [], "name": "x"}] * 5 + [{"ops": ["Open"], "name": "Gate"}]}
        bot.sent = []
        bot.chose = []
        bot.continued = 0
        bot.unsealed = []
        bot.stood = 0

        def stand(spot, timeout=500):
            bot.stood += 1
            return True

        def send(op, x, z, loc_id):
            bot.sent.append((op, x, z, loc_id))

        def pump(n=1):
            bot.world.ticks += n
            if bot.world.ticks >= 2 and bot.world.modal is None and not bot.chose:
                bot.world.modal = ("chat", 1)
            if bot.chose and bot.world.modal is not None:
                bot.world.modal = None
                bot.world.x = 3270  # teleporte de l'autre cote

        bot.stand_on_door_side = stand
        bot.send_oploc = send
        bot.pump = pump
        bot.chat_options = lambda: [("c", "Yes, ok.")] if bot.world.modal else []
        bot.chat_choose = lambda text: (bot.chose.append(text), "c")[1]
        bot.chat_continue = lambda: setattr(bot, "continued", bot.continued + 1)
        bot.unseal = lambda *key: bot.unsealed.append(key)
        bot.dbg = lambda *a: None
        return bot

    def test_paid_toll_crosses(self):
        bot = self.make_bot()
        spot = (3268, 3227, 0, 0, 0, False, 1, 1, 1, 5)
        self.assertTrue(bot._open_toll(spot, deadline=100))
        self.assertEqual(bot.chose, ["Yes, ok."])
        self.assertTrue(bot.unsealed)

    def test_no_dialog_is_refusal(self):
        bot = self.make_bot()
        bot.world.modal = None

        def pump(n=1):
            bot.world.ticks += n

        bot.pump = pump
        spot = (3268, 3227, 0, 0, 0, False, 1, 1, 1, 5)
        self.assertFalse(bot._open_toll(spot, deadline=10))


class PaidParamTest(unittest.TestCase):
    def test_explicit_paid_restores_flag(self):
        from lifescape.client.movement.walk import MovementWalk

        seen = {}

        class Probe(MovementWalk):
            def _walk_to_inner(self, *args, **kwargs):
                seen["during"] = self.allow_paid_paths
                return "done"

        bot = Probe()
        bot.allow_paid_paths = False
        bot._walk_dest = None
        self.assertEqual(bot.walk_to(1, 2, paid=True), "done")
        self.assertTrue(seen["during"])
        self.assertFalse(bot.allow_paid_paths)

    def test_none_inherits_flag(self):
        from lifescape.client.movement.walk import MovementWalk

        seen = {}

        class Probe(MovementWalk):
            def _walk_to_inner(self, *args, **kwargs):
                seen["during"] = self.allow_paid_paths
                return "done"

        bot = Probe()
        bot.allow_paid_paths = True
        bot._walk_dest = None
        self.assertEqual(bot.walk_to(1, 2), "done")
        self.assertTrue(seen["during"])
        self.assertTrue(bot.allow_paid_paths)


if __name__ == "__main__":
    unittest.main()
