import unittest
from types import SimpleNamespace


from scripts.randomevents import in_maze, in_mime, pick_triffid


def bot_at(x, z):
    return SimpleNamespace(world=SimpleNamespace(x=x, z=z))


class PickBot:
    def __init__(self, seeds):
        self.world = SimpleNamespace(x=3200, z=3200, ticks=0)
        self.seeds = seeds
        self.ops = []

    def npcs_named(self, names):
        return list(self.seeds)

    def op_npc(self, op, nid):
        self.ops.append((op, nid))

    def pump(self, ticks):
        pass


class TriffidPickTest(unittest.TestCase):
    def test_picks_nearest_seed_in_range(self):
        bot = PickBot([(7, {"x": 3202, "z": 3201}), (9, {"x": 3210, "z": 3200})])
        pick_triffid(bot)
        self.assertEqual(bot.ops, [(1, 7)])

    def test_ignores_seeds_out_of_range(self):
        bot = PickBot([(7, {"x": 3220, "z": 3200})])
        pick_triffid(bot)
        self.assertEqual(bot.ops, [])

    def test_no_seed_no_op(self):
        bot = PickBot([])
        pick_triffid(bot)
        self.assertEqual(bot.ops, [])


class RescueTest(unittest.TestCase):
    def test_maze_region(self):
        self.assertTrue(in_maze(bot_at(2892, 4586)))
        self.assertTrue(in_maze(bot_at(2911, 4575)))
        self.assertFalse(in_maze(bot_at(3222, 3222)))
        self.assertFalse(in_maze(bot_at(3091, 3243)))
        self.assertFalse(in_maze(bot_at(2880 - 1, 4575)))

    def test_mime_region(self):
        self.assertTrue(in_mime(bot_at(2008, 4764)))
        self.assertFalse(in_mime(bot_at(3222, 3222)))
        self.assertFalse(in_mime(bot_at(2892, 4586)))


class MimeButtonsTest(unittest.TestCase):
    def test_every_emote_maps_to_its_interface_button(self):
        from scripts.rescue_mime import MIME_MODAL, SEQ_TO_EMOTE, button_for
        # rs2: com_2 is emote 0 (cry) through com_9, emote 7 (glass box).
        self.assertEqual(MIME_MODAL, ("chat", 6543))
        self.assertEqual(len(SEQ_TO_EMOTE), 8)
        self.assertEqual(button_for(860), 6546)  # cry -> com_2 (comps 6546)
        self.assertEqual(button_for(857), 6547)  # think -> com_3
        self.assertEqual(button_for(1131), 6553)  # glass box -> com_9
