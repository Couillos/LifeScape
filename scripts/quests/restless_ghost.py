"""Requirement check and completion: The Restless Ghost."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as C

import gamedata as knowledge


QUEST = quest_requirements.by_key("priest")


def _saved_stage():
    """Save-based quest stage (live varps are blind to pre-login stages)."""
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return 0
    try:
        from lifescape import save as save_mod
        varps = save_mod.load_account(account).get("varps", {})
        return varps.get(QUEST.completion_varps[0][0], 0) or 0
    except Exception:
        return 0


def _wear(bot, name):
    """Wear an item through its Wield op; already-worn is fine."""
    obj = solver.obj_id(name)
    worn = bot.world.inv.get(knowledge.comps.com_id("wornitems:wear"), {})
    if any(held == obj for held, _count in worn.values()):
        return
    try:
        com, slot = C.inv_slot(bot, obj)
    except KeyError:
        raise solver.SolverError("no %s to wear at %s" % (name, bot.position))
    bot.op_held(2, obj, slot, com)
    bot.pump(10)


def solve(bot):
    # Completed stages never need re-talking (fresh logins are blind to
    # live varps, so read the persisted stage instead).
    stage = _saved_stage()
    if stage < 1:
        solver.talk_like(bot, "father_aereck", ["looking for a quest", "quest"])
    if stage < 2:
        solver.talk_like(bot, "father_urhney", ["aereck", "ghost", "sent me"])
    # The ghost only spawns when its coffin is opened, and only speaks
    # through the ghostspeak amulet: wear it first, then open, then talk.
    _wear(bot, "amulet_of_ghostspeak")
    solver.operate_loc(bot, ["shutghostcoffin"], op=1)
    solver.talk_like(bot, "ghost", ["problem", "doomed", "skull", "yep", "yes"], near=(3249, 3194))
    # The skull lies in the tower basement south-west of here; taking it
    # wakes its skeleton, which a maxed account simply tanks.
    bot.approach(3120, 9565, radius=6, timeout=600, open_doors=True)
    solver.take_near(bot, "ghostskull", timeout=300)
    solver.use_item_on_loc(bot, "ghostskull", ["restless_ghost_altar"])
    try:
        _wear(bot, "amulet_of_strength")
    except Exception:
        pass
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
