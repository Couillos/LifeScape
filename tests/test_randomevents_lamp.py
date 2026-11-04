import unittest
from types import SimpleNamespace

import gamedata as knowledge


class LampBot:
    """Minimal bot: `lamps` lamps in slot 0, modal opens on rub, confirm spends one."""

    def __init__(self, varps, lamps=1):
        self.lamp_id = knowledge.objs.by_name("macro_genilamp")[0].obj_id
        self.lamps = lamps
        self.world = SimpleNamespace(ticks=0, modal=None, messages=[], varps=varps)
        self.buttons = []

    def inventory(self):
        return {self.lamp_id: self.lamps} if self.lamps else {}

    def backpack(self):
        return 1, ({0: (self.lamp_id, self.lamps)} if self.lamps else {})

    def op_held(self, op, obj, slot, com):
        assert (op, obj, slot) == (1, self.lamp_id, 0)
        self.world.modal = ("main", 999)

    def if_button(self, com):
        self.buttons.append(com)
        self.world.ticks += 1
        if com == knowledge.com_id("xplamp:confirm"):
            self.lamps -= 1
            self.world.modal = ("chat", 999)

    def pump(self, ticks):
        self.world.ticks += ticks

    def chat_continue(self):
        self.world.modal = None

    def close_modal(self):
        self.world.modal = None


class GenieLampTest(unittest.TestCase):
    def test_quest_done_selects_runecraft(self):
        from scripts import randomevents
        bot = LampBot({63: 6})
        self.assertTrue(randomevents.lamp_in_pack(bot))
        self.assertTrue(randomevents.rub_genie_lamp(bot))
        self.assertEqual(bot.buttons[:2], [
            knowledge.com_id("xplamp:runecraft"),
            knowledge.com_id("xplamp:confirm"),
        ])

    def test_quest_missing_selects_prayer(self):
        from scripts import randomevents
        bot = LampBot({})
        self.assertTrue(randomevents.lamp_in_pack(bot))
        self.assertTrue(randomevents.rub_genie_lamp(bot))
        self.assertEqual(bot.buttons[:2], [
            knowledge.com_id("xplamp:prayer"),
            knowledge.com_id("xplamp:confirm"),
        ])

    def test_two_lamps_both_rubbed_at_once(self):
        from scripts import randomevents
        bot = LampBot({}, lamps=2)
        self.assertTrue(randomevents.rub_genie_lamp(bot))
        self.assertFalse(randomevents.lamp_in_pack(bot))
        self.assertEqual(bot.buttons.count(knowledge.com_id("xplamp:confirm")), 2)

    def test_no_lamp_is_noop(self):
        from scripts import randomevents
        bot = LampBot({}, lamps=0)
        self.assertFalse(randomevents.lamp_in_pack(bot))
        self.assertTrue(randomevents.rub_genie_lamp(bot))
        self.assertEqual(bot.buttons, [])


if __name__ == "__main__":
    unittest.main()
