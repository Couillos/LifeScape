"""Requirement check and completion: Black Knight's Fortress.

Server truth: content/scripts/quests/quest_blackknight/scripts/quest_blackknight.rs2
(Sir Amik dialogue: areas/area_falador/scripts/sir_amik_varze.rs2).

Sequence: start at Sir Amik (%spy 0->1), disguise (iron chainbody +
bronze med helm worn), south door in, banquet door (guard lets us
through on "going in anyway"), inner door, listen at the grill
(%spy 1->2), upstairs push the secret wall, use a normal cabbage on
the hole (%spy 2->3), report back to Sir Amik for 2500 coins.
"""

import os
import sys

from lifescape import save as save_mod
from lifescape.client import Unreachable

from gamedata import quest_requirements
import gamedata as knowledge
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("blackknight")

SPY = knowledge.varp_id_by_name("spy")
STARTED = 1
LISTENED = 2
SABOTAGED = 3
COMPLETE = 4

# Save-backed stage: fresh logins are blind to live varps (spy is not
# transmitted), so resume gates consult the persisted stage too.
_SAVED_VARPS = {}


def _saved_varps():
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return {}
    try:
        return save_mod.load_account(account).get("varps", {})
    except Exception:
        return {}


def _stage(bot):
    return max(bot.world.varps.get(SPY, 0), _SAVED_VARPS.get(SPY, 0))


def _count(bot, name):
    obj = solver.obj_id(name)
    carried = bot.inventory().get(obj, 0)
    worn_com = knowledge.com_id("wornitems:wear")
    for held, _count in bot.world.inv.get(worn_com, {}).values():
        if held == obj:
            carried += _count
    return carried

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
    global _SAVED_VARPS
    _SAVED_VARPS = _saved_varps()
    if _stage(bot) == COMPLETE:
        return
    # The disguise stays worn; the cabbage is eaten at stage 3, so only
    # fresh runs need one in the pack.
    if _count(bot, "iron_chainbody") < 1 or _count(bot, "bronze_med_helm") < 1:
        raise solver.SolverError("no disguise (chainbody + med helm) at %s" % (bot.position,))
    if _stage(bot) < SABOTAGED and _count(bot, "cabbage") < 1:
        raise solver.SolverError("no cabbage for the hole at %s" % (bot.position,))
    if _stage(bot) < STARTED:
        solver.talk_like(bot, "sir_amik_varze", ["quest", "laugh", "give it a go"])
    _wear(bot, "iron_chainbody")
    _wear(bot, "bronze_med_helm")
    _go_level(bot, 0)
    solver.operate_loc(bot, ("bkfortressdoor1",), 1, timeout=300)
    if bot.world.modal:
        raise solver.SolverError("fortress door refused entry: wear iron_chainbody + bronze_med_helm")
    solver.operate_loc(bot, ("bkfortressdoor2",), 1, timeout=120)
    solver.finish_dialog_like(bot, ["going in", "don't care"])
    # The inner door is upstairs (static spawns put it on level 1).
    _go_level(bot, 1)
    solver.operate_loc(bot, ("bkfortressdoor3",), 1, timeout=120)
    _listen_at_grill(bot)
    if _stage(bot) < SABOTAGED:
        _sabotage_potion(bot)
    solver.talk(bot, "sir_amik_varze")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
