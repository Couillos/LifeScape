"""Requirement check and completion: Ernest the Chicken."""

import os
import sys


from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run
from scripts.tutorial import common as tutorial_common

QUEST = quest_requirements.by_key("haunted")


def _has(bot, name):
    return bot.inventory().get(solver.obj_id(name), 0) > 0


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


def solve(bot):
    # Manor doors only open outward: never walk back out for a re-talk.
    stage = _saved_stage()
    if stage < 1:
        solver.talk_like(bot, "veronica", ["quest", "help", "ernest"])
    if stage < 2:
        solver.talk_like(bot, "professor_oddenstein", ["ernest", "looking", "chicken", "change", "back"])
    # Only the pressure gauge needs work in-world: the oil can and rubber
    # tube arrive from the bank (see QUEST items). Kill the fountain's
    # piranhas with the banked fish food + poison, then take the gauge.
    if not _has(bot, "pressure_gauge"):
        if not _has(bot, "poisoned_fish_food"):
            if not (_has(bot, "fish_food") and _has(bot, "poison")):
                raise solver.SolverError("need fish food and poison for the fountain")
            tutorial_common.use_held_on_held(bot, solver.obj_id("fish_food"), solver.obj_id("poison"))
        solver.go_to_npc(bot, "veronica")
        solver.use_item_on_loc(bot, "poisoned_fish_food", ("hauntedfountain",))
        solver.operate_loc(bot, ("hauntedfountain",), 1)
    solver.talk_like(bot, "professor_oddenstein", ["found", "everything", "gauge", "tube", "oil"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
