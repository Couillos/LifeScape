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

# Stage gates (10 started, 20 osman, 30 prep, 40 drunk, 50 tied, 100 saved).
# Save-backed: fresh logins are blind to live varps, so max wins either way.
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return {}
    try:
        from lifescape import save as save_mod
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _count(bot, obj_id):
    return bot.inventory().get(obj_id, 0)


def _stage(bot):
    return max(bot.world.varps.get(PRINCEQUEST, 0), _SAVED_VARPS.get(PRINCEQUEST, 0))


def use_item_on_npc(bot, item, npc, timeout=1800):
    """OPNPCU: the client has no use-item-on-NPC wrapper, so send it directly.

    Needed once: rope on Lady Keli. The packet is (npc slot, obj, slot, com),
    the same field order the server decodes in OpNpcUDecoder. Timeout is in
    ticks (1800 = ~3 min at 100 ms): Keli patrols, so several chase rounds
    may be needed before a send lands while adjacent.
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
        bot._dbg("opnpcu slot=%d obj=%d npc=%d,%d" % (nid, obj, person["x"], person["z"]))
        bot.connection.send(client_op_id("OPNPCU"), body.bytes())
        # An accepted use opens a result mesbox that halts the script until
        # dismissed: without a continue the tie-up never completes and later
        # sends are refused while it pends (the earlier 18 s timeout mistook
        # this for a moving-NPC problem).
        wait_ticks(bot, 10)
        solver._drain_dialog(bot)
        bot.pump(5)
    raise solver.SolverError("could not use %s on %s at %s" % (item, npc, bot.position))


def solve(bot):
    if bot.world.varps.get(PRINCEQUEST) == COMPLETE:
        return
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    stage = _stage(bot)
    # Hassan starts the quest; Osman explains the two jobs (rope/disguise
    # via Leela, key copy via Keli's boast). Keywords pick "first thing"
    # then "better go" so the briefing ends instead of looping.
    if stage < 10:
        solver.talk_like(bot, "hassan", ["help", "chancellor", "osman"])
        stage = _stage(bot)
    if stage < 20:
        solver.talk_like(bot, "osman", ["better", "okay", "luck", "leela", "first", "second"])
        stage = _stage(bot)
    # Leela confirms the full kit (key, wig, skirt, paste) is carried and
    # advances to prep_finished; with the kit this is a single chat.
    if stage < 30:
        solver.talk_like(bot, "leela", ["help", "free", "prince", "equipment", "guard"])
        stage = _stage(bot)
    # Joe's beer dialogue drinks all 3 beers by itself and sets guard_drunk.
    if stage < 40:
        if _count(bot, solver.obj_id("beer")) < 3:
            raise solver.SolverError("need 3 beers for Joe at %s" % (bot.position,))
        solver.talk_like(bot, "joe", ["beer", "fancy", "throat", "thirsty"])
        stage = _stage(bot)
    if stage < 40:
        # A mistargeted chat leaves the beers untouched: retry once, then
        # fail fast instead of timing out on Keli (she refuses while sober).
        solver.talk_like(bot, "joe", ["beer", "fancy", "throat", "thirsty"])
        stage = _stage(bot)
    if stage < 40:
        raise solver.SolverError("Joe did not drink the beers at %s" % (bot.position,))
    if stage < 100:
        _dx, _dz, _dlvl, _dloc = C.find_loc(bot, "alidoor")
        if not bot.can_reach(_dx, _dz - 1, _dlvl, near=0):
            # Locked (or Keli back within 10 tiles): tie her if she is
            # about — the tie consumes the rope — then turn the key.
            if bot.npcs_named(("lady_keli",)):
                if _count(bot, solver.obj_id("rope")) < 1:
                    raise solver.SolverError("need rope for Keli at %s" % (bot.position,))
                use_item_on_npc(bot, "rope", "lady_keli")
                if _count(bot, solver.obj_id("rope")) > 0:
                    raise solver.SolverError("Keli refused the rope at %s" % (bot.position,))
            solver.use_item_on_loc(bot, "princeskey", ("alidoor",))
            if not bot.can_reach(_dx, _dz - 1, _dlvl, near=0):
                raise solver.SolverError("cell door refused (Keli respawned?) at %s" % (bot.position,))
        solver.talk_like(bot, "prince_ali_prison", ["rescue", "disguise", "key", "leela"],
                         near=(_dx, _dz + 1))
        stage = _stage(bot)
    if stage < 100:
        raise solver.SolverError("prince not freed at %s" % (bot.position,))
    # Hassan pays out 700 coins, which completes the quest.
    if stage < 110:
        solver.talk_like(bot, "hassan", ["reward", "payment", "rescued", "emir"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
