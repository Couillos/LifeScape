"""Requirement check and completion: Prince Ali Rescue."""

import os
import sys


import gamedata as knowledge
from gamedata import quest_requirements
from lifescape import save as save_mod
from lifescape.client import Unreachable
from lifescape.waits import wait_ticks
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as tutorial_common

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
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _count(bot, obj_id):
    return bot.inventory().get(obj_id, 0)


def _stage(bot):
    return max(bot.world.varps.get(PRINCEQUEST, 0), _SAVED_VARPS.get(PRINCEQUEST, 0))


def _await_stage(bot, want, timeout=60):
    """Poll a stage gate: the live varp update can land a tick after the
    dialogue that set it closes, and a single read falsely fails."""
    target = bot.world.ticks + timeout
    while bot.world.ticks < target and _stage(bot) < want:
        bot.pump(2)
    return _stage(bot)


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
            com, slot = tutorial_common.inv_slot(bot, obj)
        except KeyError:
            raise solver.SolverError("no %s for %s at %s" % (item, npc, bot.position))
        bot.op_npc_u(nid, obj, slot, com)
        # An accepted use opens a result mesbox that halts the script until
        # dismissed: without a continue the tie-up never completes and later
        # sends are refused while it pends (the earlier 18 s timeout mistook
        # this for a moving-NPC problem).
        wait_ticks(bot, 10)
        solver.drain_dialog(bot)
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
        stage = _await_stage(bot, 10)
    if stage < 20:
        solver.talk_like(bot, "osman", ["better", "okay", "luck", "leela", "first", "second"])
        stage = _await_stage(bot, 20)
    # Leela confirms the full kit (key, wig, skirt, paste) is carried and
    # advances to prep_finished; with the kit this is a single chat.
    if stage < 30:
        missing = [name for name in ("princeskey", "blondwig", "pink_skirt", "skinpaste")
                   if _count(bot, solver.obj_id(name)) < 1]
        if missing:
            raise solver.SolverError("need %s for Leela at %s" % (",".join(missing), bot.position))
        solver.talk_like(bot, "leela", ["help", "free", "prince", "equipment", "guard"])
        stage = _await_stage(bot, 30)
    # Joe's beer dialogue drinks all 3 beers by itself and sets guard_drunk.
    if stage < 40:
        if _count(bot, solver.obj_id("beer")) < 3:
            raise solver.SolverError("need 3 beers for Joe at %s" % (bot.position,))
        solver.talk_like(bot, "joe", ["beer", "fancy", "throat", "thirsty"])
        stage = _await_stage(bot, 40)
    if stage < 40:
        # A mistargeted chat leaves the beers untouched: retry once, then
        # fail fast instead of timing out on Keli (she refuses while sober).
        solver.talk_like(bot, "joe", ["beer", "fancy", "throat", "thirsty"])
        stage = _await_stage(bot, 40)
    if stage < 40:
        raise solver.SolverError("Joe did not drink the beers at %s" % (bot.position,))
    if stage < 100:
        _dx, _dz, _dlvl, _dloc = tutorial_common.find_loc(bot, "alidoor")
        # Fresh logins are blind to NPCs until the stream arrives: wait for
        # Keli before deciding the tie is unnecessary (a respawned Keli the
        # client never sees still blocks the door server-side).
        target = bot.world.ticks + 150
        while bot.world.ticks < target and not bot.npcs_named(("lady_keli",)):
            bot.pump(5)
        # Tie + key, driven by the server's own verdict: the door answers
        # "You unlock the door." on success and names Keli on refusal (both
        # plain game messages, no modal). A refused tie keeps the rope, so
        # a flicker-exited tie simply retries with the rope still held.
        key = solver.obj_id("princeskey")
        if _count(bot, key) < 1:
            raise solver.SolverError("need princeskey for the cell at %s" % (bot.position,))
        unlocked = False
        for _attempt in range(4):
            if bot.npcs_named(("lady_keli",)):
                if _count(bot, solver.obj_id("rope")) < 1:
                    raise solver.SolverError("need rope for Keli at %s" % (bot.position,))
                use_item_on_npc(bot, "rope", "lady_keli")
                target = bot.world.ticks + 30
                while _count(bot, solver.obj_id("rope")) > 0 and bot.world.ticks < target:
                    bot.pump(2)
            seen_msgs = len(bot.world.messages)
            # The door's script silently refuses clicks resolved from the
            # north/east (coordz guard), so stand south first: the server
            # then fires from the only side it accepts. Still no full
            # client approach — the walk south is a couple of tiles, keeping
            # well inside the ~10 s Keli respawn window.
            try:
                bot.walk_to(_dx, _dz + 1, _dlvl, timeout=15, open_doors=True)
            except (Unreachable, TimeoutError):
                pass
            # No client-side approach beyond that: the server walks into
            # reach itself, while a longer client walk burns the respawn
            # window.
            tutorial_common.use_held_on_loc(bot, _dx, _dz, _dloc, key)
            target = bot.world.ticks + 100
            while bot.world.ticks < target and not unlocked:
                bot.pump(5)
                for text in bot.world.messages[seen_msgs:]:
                    if "unlock the door" in text:
                        unlocked = True
                    elif "Keli" in text:
                        break
                    else:
                        print("door click says: %r" % (text[:80],), flush=True)
                else:
                    continue
                break
            if unlocked:
                break
        if not unlocked:
            raise solver.SolverError("cell door refused at %s" % (bot.position,))
        # Talk from the door tile: with the gate open there is no wall in
        # the way. (No step-in: walking onto the tile grinds when shut and
        # eats the ~60 s the open gate lasts.)
        solver.talk_like(bot, "prince_ali_prison", ["rescue", "disguise", "key", "leela"],
                         near=(_dx, _dz + 1))
        stage = _await_stage(bot, 100)
    if stage < 100:
        raise solver.SolverError("prince not freed at %s" % (bot.position,))
    # Hassan pays out 700 coins, which completes the quest.
    if stage < 110:
        solver.talk_like(bot, "hassan", ["reward", "payment", "rescued", "emir"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
