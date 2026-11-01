"""Action payloads are byte-exact without needing the server."""

import unittest

from lifescape import protocol
from lifescape.client import Bot
from lifescape.world import to_base37


class StubConnection:
    def __init__(self):
        self.sent = []

    def send(self, op_id, payload=b""):
        self.sent.append((op_id, payload))


def bot_with_stub():
    bot = Bot()
    bot.connection = StubConnection()
    return bot


class ActionsTest(unittest.TestCase):
    def test_op_loc_layout(self):
        bot = bot_with_stub()
        bot.op_loc(1, 3129, 3106, 1516)
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("OPLOC1"))
        self.assertEqual(payload, bytes([0x0C, 0x39, 0x0C, 0x22, 0x05, 0xEC]))

    def test_op_npc_layout(self):
        bot = bot_with_stub()
        bot.op_npc(1, 7)
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("OPNPC1"))
        self.assertEqual(payload, bytes([0x00, 0x07]))

    def test_if_button_layout(self):
        bot = bot_with_stub()
        bot.if_button(0x1234)
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("IF_BUTTON"))
        self.assertEqual(payload, bytes([0x12, 0x34]))

    def test_chat_continue_sends_two_ignored_bytes(self):
        bot = bot_with_stub()
        bot.chat_continue()
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("RESUME_PAUSEBUTTON"))
        self.assertEqual(payload, bytes([0, 0]))

    def test_cheat_strips_colons(self):
        bot = bot_with_stub()
        bot.cheat("::give logs 1")
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("CLIENT_CHEAT"))
        self.assertEqual(payload, b"give logs 1\n")

    def test_op_held_u_layout(self):
        bot = bot_with_stub()
        bot.op_held_u(590, 3, 3214, 2511, 7, 3214)
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("OPHELDU"))
        self.assertEqual(payload, bytes([0x02, 0x4E, 0x00, 0x03, 0x0C, 0x8E, 0x09, 0xCF, 0x00, 0x07, 0x0C, 0x8E]))

    def test_tut_clickside_layout(self):
        bot = bot_with_stub()
        bot.tut_clickside(3)
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("TUT_CLICKSIDE"))
        self.assertEqual(payload, bytes([3]))

    def test_friend_add_uses_name37(self):
        bot = bot_with_stub()
        bot.friend_add("t08")
        op, payload = bot.connection.sent[-1]
        self.assertEqual(op, protocol.client_op_id("FRIENDLIST_ADD"))
        self.assertEqual(int.from_bytes(payload, "big"), to_base37("t08"))
        self.assertEqual(len(payload), 8)

    def test_base37_roundtrip(self):
        self.assertEqual(to_base37("t08"), 0x6EFE)


if __name__ == "__main__":
    unittest.main()
