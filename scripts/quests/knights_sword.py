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

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

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


def _stage(bot):
    return bot.world.varps.get(SQUIRE, 0)


def _count(bot, name):
    return bot.inventory().get(solver.obj_id(name), 0)


def _talk_until(bot, name, keywords, want, tries=3):
    """Talk until the squire varp reaches want; a wrong option otherwise cascades."""
    for _ in range(tries):
        if _stage(bot) >= want:
            return
        solver.talk_like(bot, name, keywords)
    if _stage(bot) < want:
        raise solver.SolverError("talking to %s never reached stage %d (at %d)" % (name, want, _stage(bot)))


def _portrait(bot):
    if _count(bot, "knights_portrait") > 0:
        return
    bot.intend("climbing to Vyvin's room for the portrait")
    bot.approach(*VYVIN_ROOM, radius=4, timeout=600, open_doors=True)
    target = bot.world.ticks + 600
    while bot.world.ticks < target:
        if _count(bot, "knights_portrait") > 0:
            return
        try:
            solver.operate_loc(bot, ("vyvincupboardshut",), 1, timeout=60)
        except Exception as error:
            print("cupboard open failed: %s" % (error,), flush=True)
        try:
            solver.operate_loc(bot, ("vyvincupboardopen",), 1, timeout=60)
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
    # Squire starts the quest down the "make a new sword" branch.
    _talk_until(bot, "squire", ["life as a squire", "make a new sword", "dwarves make another", "give it a go"], STARTED)
    # Reldo names the Imcando and their love of redberry pie.
    _talk_until(bot, "reldo", ["imcando"], SPOKEN_RELDO)
    # Thurgo only listens after pie: the banked pie was withdrawn already.
    if _count(bot, "redberry_pie") < 1:
        raise solver.SolverError("no redberry pie for Thurgo at %s" % (bot.position,))
    _talk_until(bot, "thurgo", ["redberry pie"], GIVEN_PIE)
    _talk_until(bot, "thurgo", ["special sword"], SPOKEN_THURGO)
    # Back to the squire: he points at Vyvin's cupboard portrait.
    _talk_until(bot, "squire", ["picture", "portrait", "sword"], LOOKING_PORTRAIT)
    _portrait(bot)
    # Portrait for the design, then ore + bars for the blade.
    _talk_until(bot, "thurgo", ["sword", "picture"], LOOKING_BLURITE)
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
