"""Requirement check and completion: Black Knight's Fortress.

Server truth: content/scripts/quests/quest_blackknight/scripts/quest_blackknight.rs2
(Sir Amik dialogue: areas/area_falador/scripts/sir_amik_varze.rs2).

Sequence: start at Sir Amik (%spy 0->1), disguise (iron chainbody +
bronze med helm worn), south door in, banquet door (guard lets us
through on "going in anyway"), inner door, listen at the grill
(%spy 1->2), upstairs push the secret wall, use a normal cabbage on
the hole (%spy 2->3), report back to Sir Amik for 2500 coins.
"""

import sys


from lifescape.client import Unreachable

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("blackknight")

GRILL_LEVEL = 0  # "Grate 3025 3508 0" header in quest_blackknight.rs2
HOLE_LEVEL = 1  # "Hole 3031 3508 1" header in quest_blackknight.rs2


def _wear(bot, name):
    """Wear a disguise piece; already-worn is fine (withdraw proved it exists)."""
    obj = solver.obj_id(name)
    try:
        slot = solver.find_item_slot(bot, obj)
    except solver.SolverError:
        return
    com, _slots = bot.backpack()
    bot.op_held(2, obj, slot, com)
    bot.pump(10)


def _climb_toward(bot, level):
    if bot.world.level > level:
        solver.operate_loc(bot, ("laddertop", "spiralstairstop", "stairstop"), 1, timeout=120)
    elif bot.world.level < level:
        solver.operate_loc(bot, ("ladder", "spiralstairs", "stairs"), 1, timeout=120)
    else:
        raise solver.SolverError("no path on level %d at %s" % (level, bot.position))


def _go_level(bot, level):
    for _ in range(6):
        if bot.world.level == level:
            return
        _climb_toward(bot, level)
    raise solver.SolverError("cannot reach level %d from %s" % (level, bot.position))


def _listen_at_grill(bot):
    for _ in range(4):
        try:
            solver.operate_loc(bot, ("witchgrill",), 1, timeout=120)
            break
        except Unreachable:
            _climb_toward(bot, GRILL_LEVEL)
    else:
        raise solver.SolverError("cannot reach the fortress grill from %s" % (bot.position,))
    solver.finish_dialog_like(bot, ["cabbage", "potion", "draynor"])


def _sabotage_potion(bot):
    for _ in range(4):
        if bot.world.level == HOLE_LEVEL:
            break
        _climb_toward(bot, HOLE_LEVEL)
    try:
        solver.use_item_on_loc(bot, "cabbage", ("blackknighthole",))
    except Unreachable:
        # Hole room hides behind the secret wall: push it, then retry.
        solver.operate_loc(bot, ("bksecretdoor",), 1, timeout=120)
        solver.use_item_on_loc(bot, "cabbage", ("blackknighthole",))
    solver.finish_dialog_like(bot, ["sabotaged", "successfully"])


def solve(bot):
    solver.talk_like(bot, "sir_amik_varze", ["quest", "laugh", "give it a go"])
    _wear(bot, "iron_chainbody")
    _wear(bot, "bronze_med_helm")
    _go_level(bot, 0)
    solver.operate_loc(bot, ("bkfortressdoor1",), 1, timeout=300)
    if bot.world.modal:
        raise solver.SolverError("fortress door refused entry: wear iron_chainbody + bronze_med_helm")
    solver.operate_loc(bot, ("bkfortressdoor2",), 1, timeout=120)
    solver.finish_dialog_like(bot, ["going in", "don't care"])
    solver.operate_loc(bot, ("bkfortressdoor3",), 1, timeout=120)
    _listen_at_grill(bot)
    _sabotage_potion(bot)
    solver.talk(bot, "sir_amik_varze")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
