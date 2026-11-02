"""Requirement check and completion: The Knight's Sword.

Squire (Falador) -> Reldo (Varrock library) -> Thurgo (Mudskipper Point,
hand over the redberry pie) -> Squire (learn about the portrait) ->
Vyvin's cupboard upstairs in Falador castle (wait Vyvin out) -> Thurgo
(portrait, then blurite ore + 2 iron bars for the sword) -> Squire.

The requirement check already proves mining 10 plus the pie, the ore
and the bars, so this only delivers; no mining here.
"""

import os
import sys


import gamedata as knowledge
from gamedata import quest_requirements
from lifescape.waits import wait_ticks
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("squire")

SQUIRE = knowledge.varp_id_by_name("squire")
STARTED = 1
SPOKEN_RELDO = 2
GIVEN_PIE = 3
SPOKEN_THURGO = 4
LOOKING_PORTRAIT = 5
LOOKING_BLURITE = 6
COMPLETE = 7

VYVIN_ROOM = (2983, 3335, 2)

# The cupboard only answers from its south side: approach() parks west
# (2983,3336), which the server refuses with "I can't reach that!".
CUPBOARD_STAND = (2984, 3335, 2)

# Save-backed stages for this run: fresh logins are blind to live varps,
# so gates consult the persisted stage too (max wins either way).
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


def _await_stage(bot, want, timeout=60):
    """Poll a stage gate: the live varp update can land a tick after the
    dialogue that set it closes, and a single read falsely fails."""
    target = bot.world.ticks + timeout
    while bot.world.ticks < target and _stage(bot) < want:
        bot.pump(2)
    return _stage(bot)


def _stage(bot):
    return max(bot.world.varps.get(SQUIRE, 0), _SAVED_VARPS.get(SQUIRE, 0))


def _count(bot, name):
    return bot.inventory().get(solver.obj_id(name), 0)


def _talk_until(bot, name, keywords, want, tries=3):
    """Talk until the squire varp reaches want; a wrong option otherwise cascades."""
    for _ in range(tries):
        if _await_stage(bot, want) >= want:
            return
        solver.talk_like(bot, name, keywords)
        if _await_stage(bot, want) >= want:
            return
    if _await_stage(bot, want) < want:
        raise solver.SolverError("talking to %s never reached stage %d (at %d)" % (name, want, _stage(bot)))


def _vyvin_close(bot):
    """True while Vyvin guards the cupboard (the server checks radius 1).

    The anchor is ambiguous (cupboard or player), so both must be clear.
    """
    for _nid, npc in bot.npcs_named(("sir_vyvin",)):
        if max(abs(npc["x"] - 2984), abs(npc["z"] - 3336)) <= 1:
            return True
        if max(abs(npc["x"] - bot.world.x), abs(npc["z"] - bot.world.z)) <= 1:
            return True
    return False


def _portrait(bot):
    if _stage(bot) >= LOOKING_BLURITE:
        return
    if _count(bot, "knights_portrait") > 0:
        return
    bot.intend("climbing to Vyvin's room for the portrait")
    bot.approach(*VYVIN_ROOM, radius=4, timeout=600, open_doors=True)
    target = bot.world.ticks + 1800
    while bot.world.ticks < target:
        if _count(bot, "knights_portrait") > 0:
            return
        if _vyvin_close(bot):
            wait_ticks(bot, 10)
            continue
        try:
            solver.operate_loc(bot, ("vyvincupboardshut",), 1, timeout=60, stand=CUPBOARD_STAND)
        except Exception as error:
            print("cupboard open failed: %s" % (error,), flush=True)
        if _vyvin_close(bot):
            # He wandered back while opening: skip the search (HEY costs a
            # cycle) and wait him out instead.
            wait_ticks(bot, 10)
            continue
        try:
            solver.operate_loc(bot, ("vyvincupboardopen",), 1, timeout=60, stand=CUPBOARD_STAND)
        except Exception as error:
            print("cupboard search failed: %s" % (error,), flush=True)
        wait_ticks(bot, 5)
        if _count(bot, "knights_portrait") > 0:
            return
        # Either Vyvin caught us (HEY! ...) or the room is busy: back off
        # and let him wander away from the cupboard before retrying.
        print("portrait not yet taken at %s, waiting Vyvin out" % (bot.position,), flush=True)
        wait_ticks(bot, 30)
    raise solver.SolverError("could not take the portrait at %s" % (bot.position,))


def solve(bot):
    if _stage(bot) == COMPLETE:
        return
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    # Squire starts the quest down the "make a new sword" branch.
    _talk_until(bot, "squire", ["life as a squire", "make a new sword", "dwarves make another", "give it a go"], STARTED)
    # Reldo names the Imcando and their love of redberry pie.
    _talk_until(bot, "reldo", ["imcando"], SPOKEN_RELDO)
    # Thurgo only listens after pie: the banked pie was withdrawn already.
    # The pie is eaten on delivery, so only fresh runs need one.
    if _stage(bot) < GIVEN_PIE and _count(bot, "redberry_pie") < 1:
        raise solver.SolverError("no redberry pie for Thurgo at %s" % (bot.position,))
    _talk_until(bot, "thurgo", ["redberry pie"], GIVEN_PIE)
    _talk_until(bot, "thurgo", ["special sword"], SPOKEN_THURGO)
    # Back to the squire: he points at Vyvin's cupboard portrait.
    _talk_until(bot, "squire", ["picture", "portrait", "sword"], LOOKING_PORTRAIT)
    _portrait(bot)
    # Portrait for the design, then ore + bars for the blade. The bars and
    # ore are eaten when the sword is made, so only fresh blades need them.
    _talk_until(bot, "thurgo", ["sword", "picture"], LOOKING_BLURITE)
    if _count(bot, "faladian_sword") < 1:
        if _count(bot, "blurite_ore") < 1 or _count(bot, "iron_bar") < 2:
            raise solver.SolverError("missing sword materials at %s" % (bot.position,))
        solver.talk_like(bot, "thurgo", ["sword", "materials", "here"])
    if _count(bot, "faladian_sword") < 1:
        raise solver.SolverError("Thurgo never made the sword at %s" % (bot.position,))
    # The squire only accepts the blade from the backpack, never worn.
    solver.talk_like(bot, "squire", ["sword", "thank", "hurry"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
