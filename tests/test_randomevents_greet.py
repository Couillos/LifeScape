import unittest
from types import SimpleNamespace


class GreetBot:
    """Fake bot with a dwarf on the diagonal tile: approach() would stop
    there, approach_npc() steps orthogonal first."""

    def __init__(self):
        self.world = SimpleNamespace(x=3200, z=3200, level=0, ticks=0,
                                     modal=None, messages=[],
                                     npcs={7: {"x": 3201, "z": 3201, "level": 0, "type": 956}})
        self.calls = []

    def find_npc(self, name):
        return 7 if name == "macro_dwarf" else None

    def approach(self, *args, **kwargs):
        self.calls.append("approach")
        raise AssertionError("greet must not use Manhattan approach")

    def approach_npc(self, slot, timeout=3000, open_doors=True):
        self.calls.append(("approach_npc", slot))
        self.world.x, self.world.z = 3201, 3200  # orthogonal step

    def op_npc(self, op, slot):
        self.calls.append(("op_npc", op, slot))

    def pump(self, count=1):
        self.world.ticks += count

    def chat_texts(self):
        return []


class GreetTest(unittest.TestCase):
    def test_greet_steps_orthogonal_before_click(self):
        from scripts import randomevents
        bot = GreetBot()
        randomevents.greet(bot)
        self.assertIn(("approach_npc", 7), bot.calls)
        self.assertIn(("op_npc", 1, 7), bot.calls)
        self.assertNotIn("approach", bot.calls)

    def test_greet_near_skips_diagonal_without_walking(self):
        from scripts import randomevents
        bot = GreetBot()
        randomevents.greet_near(bot)
        # diagonal (1,1): no click, and no walk (pump-hook safe)
        self.assertEqual(bot.calls, [])
        self.assertEqual((bot.world.x, bot.world.z), (3200, 3200))

    def test_greet_near_clicks_orthogonal(self):
        from scripts import randomevents
        bot = GreetBot()
        bot.world.npcs[7]["x"], bot.world.npcs[7]["z"] = 3201, 3200
        randomevents.greet_near(bot)
        self.assertIn(("op_npc", 1, 7), bot.calls)


if __name__ == "__main__":
    unittest.main()
