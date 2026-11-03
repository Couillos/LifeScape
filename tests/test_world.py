"""World parsing validated on a real captured login."""

import json
import os
import unittest

from lifescape.buffer import Reader
from lifescape.world import World, base37_name
from lifescape.client import Bot

HERE = os.path.dirname(os.path.abspath(__file__))


def frames():
    with open(os.path.join(HERE, "data", "cap_t08.json")) as handle:
        return json.load(handle)


class WorldTest(unittest.TestCase):
    def test_login_lands_on_tutorial_island(self):
        world = World()
        world.feed(231, "REBUILD_NORMAL", bytes([0x01, 0x82, 0x01, 0x84]))
        for frame in frames():
            world.feed(frame["op"], frame["name"], bytes.fromhex(frame["hex"]))
        self.assertEqual(world.position, (0, 3094, 3106))

    def test_seen_players_have_names(self):
        world = World()
        for frame in frames():
            world.feed(frame["op"], frame["name"], bytes.fromhex(frame["hex"]))
        names = [entry.get("name") for entry in world.players.values()]
        self.assertTrue(all(name for name in names) or not names)

    def test_appearance_block_stays_within_its_length(self):
        # Real bytes: mask 0x01, length 0x33, 51-byte appearance, then the
        # next player's empty mask. Reading one byte short of the block made
        # every following mask land inside the appearance data.
        world = World()
        for pid in (3, 4):
            world.players[pid] = {"x": 0, "z": 0, "level": 0, "jump": 0, "name": None}
        stream = bytes.fromhex(
            "01"
            "33"
            "000000000000011200011a012401000121012a010a000000000003280337033303340335033603380000000000006ef9030000"
            "00"
        )
        data = Reader(stream)
        world._read_mask_set(data, 3, player=True)
        self.assertEqual(world.players[3]["name"], "t03")
        world._read_mask_set(data, 4, player=True)
        self.assertEqual(data.pos, len(stream))

    def test_base37(self):
        self.assertEqual(base37_name(0x6EFE), "t08")

    def test_npc_damage2_and_spotanim_sizes(self):
        world = World()
        world.npcs[7] = {"type": 100, "x": 0, "z": 0, "level": 0, "name": None}
        data = Reader(bytes([0x41, 1, 2, 3, 4, 0x12, 0x34, 0, 1, 0, 2]))
        world._read_mask_set(data, 7, player=False)
        self.assertEqual(data.pos, 11)

    def test_npc_change_type_updates_type(self):
        world = World()
        world.npcs[7] = {"type": 100, "x": 0, "z": 0, "level": 0, "name": None}
        data = Reader(bytes([0x20, 0x03, 0xAF]))
        world._read_mask_set(data, 7, player=False)
        self.assertEqual(world.npcs[7]["type"], 943)

    def test_npc_anim_updates_anim(self):
        # Mask 0x02 (ANIM): u16 seq + u8 delay. The mime rescue reads the
        # emote the mime plays from here; the block size is unchanged.
        world = World()
        world.npcs[7] = {"type": 100, "x": 0, "z": 0, "level": 0, "name": None}
        data = Reader(bytes([0x02, 0x03, 0x6C, 0x00]))
        world._read_mask_set(data, 7, player=False)
        self.assertEqual(world.npcs[7]["anim"], 876)
        self.assertEqual(data.pos, 4)

    def test_player_anim_still_skipped(self):
        world = World()
        world.players[3] = {"x": 0, "z": 0, "level": 0, "jump": 0, "name": None}
        data = Reader(bytes([0x02, 0x03, 0x6C, 0x00]))
        world._read_mask_set(data, 3, player=True)
        self.assertNotIn("anim", world.players[3])
        self.assertEqual(data.pos, 4)

    def test_npcs_survive_rebuild(self):
        world = World()
        world.npcs[7] = {"type": 100, "x": 3100, "z": 3095, "level": 0, "name": None}
        world.npc_order.append(7)
        world.feed(231, "REBUILD_NORMAL", bytes([0x01, 0x82, 0x01, 0x84]))
        world.feed(197, "NPC_INFO", bytes([0x01, 0x00]))
        self.assertIn(7, world.npcs)
        self.assertEqual(world.npcs[7]["x"], 3100)

    def test_hint_arrow_vectors(self):
        world = World()
        world.feed(156, "HINT_ARROW", bytes.fromhex("030c1a0c2380"))
        self.assertEqual(world.hint, ("coord", 3098, 3107, 128))
        world.feed(156, "HINT_ARROW", bytes.fromhex("0110a5000000"))
        self.assertEqual(world.hint, ("npc", 4261))
        world.feed(156, "HINT_ARROW", bytes.fromhex("ff0000000000"))
        self.assertIsNone(world.hint)

    def test_tut_flash_tab(self):
        world = World()
        world.feed(90, "TUT_FLASH", bytes([3]))
        self.assertEqual(world.side_flash, 3)

    def test_teleport_clears_tracked_npcs(self):
        world = World()
        world.base_x = 0
        world.base_z = 0
        world.x = 10
        world.z = 10
        world.npcs[7] = {"type": 100, "x": 10, "z": 10, "level": 0, "name": None}
        world.npc_order.append(7)
        world.feed(167, "PLAYER_INFO", bytes.fromhex("e64c8000"))
        self.assertEqual(world.npcs, {})
        self.assertEqual(world.npc_order, [])

    def test_short_hop_keeps_tracked_npcs(self):
        world = World()
        world.base_x = 0
        world.base_z = 0
        world.x = 10
        world.z = 10
        world.npcs[7] = {"type": 100, "x": 10, "z": 10, "level": 0, "name": None}
        world.npc_order.append(7)
        world.feed(167, "PLAYER_INFO", bytes.fromhex("e0a14000"))
        self.assertIn(7, world.npcs)

    def test_short_player_info_without_mask_byte(self):
        # Own update + empty known list leave 5 bits: the padded end is the
        # whole body, so no mask byte may be read past it.
        world = World()
        world.feed(167, "PLAYER_INFO", bytes([0x80, 0x00]))

    def test_big_mask_header_is_little_endian(self):
        # Server writes ip2(masks | BIG): 0x0304 arrives as 84 03 (low first).
        world = World()
        data = Reader(bytes([0x84, 0x03]) + bytes(17))
        world._read_mask_set(data, "own", True)
        self.assertEqual(data.pos, 19)

    def test_live_ground_shadows_static_template(self):
        world = World()
        world.static_objs[((3224, 3202, 0), 946)] = 1
        world.objs[((3224, 3202, 0), 946)] = 1
        self.assertTrue(world.ground_items(3224, 3202, 0, obj_id=946))
        world.objs[((3224, 3202, 0), 946)] = None
        self.assertFalse(world.ground_items(3224, 3202, 0, obj_id=946))
        world.objs[((3224, 3202, 0), 946)] = 1
        self.assertTrue(world.ground_items(3224, 3202, 0, obj_id=946))


    def test_update_stat_wire_order(self):
        # Server UpdateStatEncoder writes stat u8, exp u32, level u8.
        world = World()
        world.feed(105, "UPDATE_STAT", bytes([8, 0, 0, 3, 232, 7]))
        self.assertEqual(world.stats.get(8), (7, 1000))

    def test_inv_full_records_capacity(self):
        world = World()
        body = bytes([0x0C, 0x8E, 28]) + bytes([0, 0, 0]) * 28
        world.feed(106, "UPDATE_INV_FULL", body)
        self.assertEqual(world.inv_cap.get(0x0C8E), 28)
        self.assertEqual(world.inv.get(0x0C8E), {})

    def test_backpack_prefers_capacity_28(self):
        bot = Bot()
        bot.world.inv = {1688: {}, 3214: {0: (1351, 1)}}
        bot.world.inv_cap = {1688: 14, 3214: 28}
        com, _slots = bot.backpack()
        self.assertEqual(com, 3214)


if __name__ == "__main__":
    unittest.main()
