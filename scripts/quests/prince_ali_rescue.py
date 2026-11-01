"""Requirement check and completion: Prince Ali Rescue."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.buffer import Writer
from lifescape.client import Unreachable
from lifescape.protocol import client_op_id
from lifescape.waits import wait_ticks
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as C

QUEST = quest_requirements.by_key("prince")

PRINCEQUEST = knowledge.varp_id_by_name("princequest")
COMPLETE = 110  # ^prince_complete in content/scripts/general/configs/quest.constant


def use_item_on_npc(bot, item, npc, timeout=180):
    """OPNPCU: the client has no use-item-on-NPC wrapper, so send it directly.

    Needed once: rope on Lady Keli. The packet is (npc slot, obj, slot, com),
    the same field order the server decodes in OpNpcUDecoder.
    """
    obj = solver.obj_id(item)
    solver.go_to_npc(bot, npc)
    seen = False
    target = bot.world.ticks + timeout
    while bot.world.ticks < target:
        if not bot.npcs_named((npc,)):
            if seen:
                return True
            bot.pump(5)
            continue
        seen = True
        nid = bot.find_npc(npc)
        if nid is None:
            bot.pump(5)
            continue
        person = bot.world.npcs[nid]
        try:
            bot.approach(person["x"], person["z"], radius=1, timeout=30, open_doors=True)
        except (Unreachable, TimeoutError):
            pass
        try:
            com, slot = C.inv_slot(bot, obj)
        except KeyError:
            raise solver.SolverError("no %s for %s at %s" % (item, npc, bot.position))
        body = Writer()
        body.u16(nid)
        body.u16(obj)
        body.u16(slot)
        body.u16(com)
        bot.connection.send(client_op_id("OPNPCU"), body.bytes())
        wait_ticks(bot, 10)
    raise solver.SolverError("could not use %s on %s at %s" % (item, npc, bot.position))


def solve(bot):
    if bot.world.varps.get(PRINCEQUEST) == COMPLETE:
        return
    # Hassan starts the quest; Osman explains the two jobs (rope/disguise
    # via Leela, key copy via Keli's boast). Keywords pick "first thing"
    # then "better go" so the briefing ends instead of looping.
    solver.talk_like(bot, "hassan", ["help", "chancellor", "osman"])
    solver.talk_like(bot, "osman", ["better", "okay", "luck", "leela", "first", "second"])
    # Leela confirms the full kit (key, wig, skirt, paste) is carried and
    # advances to prep_finished; with the kit this is a single chat.
    solver.talk_like(bot, "leela", ["help", "free", "prince", "equipment", "guard"])
    # Joe's beer dialogue drinks all 3 beers by itself and sets guard_drunk.
    solver.talk_like(bot, "joe", ["beer", "fancy", "throat", "thirsty"])
    # Tie up Keli, unlock the cell door, hand over key + disguise.
    use_item_on_npc(bot, "rope", "lady_keli")
    solver.use_item_on_loc(bot, "princeskey", ("alidoor",))
    solver.talk_like(bot, "prince_ali_prison", ["rescue", "disguise", "key", "leela"])
    # Hassan pays out 700 coins, which completes the quest.
    solver.talk_like(bot, "hassan", ["reward", "payment", "rescued", "emir"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
