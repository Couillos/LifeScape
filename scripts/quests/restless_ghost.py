"""Requirement check and completion: The Restless Ghost."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

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


def solve(bot):
    # Completed stages never need re-talking (fresh logins are blind to
    # live varps, so read the persisted stage instead).
    stage = _saved_stage()
    if stage < 1:
        solver.talk_like(bot, "father_aereck", ["looking for a quest", "quest"])
    if stage < 2:
        solver.talk_like(bot, "father_urhney", ["aereck", "ghost", "sent me"])
    # The generic "ghost" also exists on other maps: stay in the graveyard.
    solver.talk_like(bot, "ghost", ["problem", "doomed", "skull", "yep", "yes"], near=(3249, 3194))
    solver.operate_loc(bot, ["shutghostcoffin"], op=1)
    solver.take_near(bot, "ghostskull")
    solver.use_item_on_loc(bot, "ghostskull", ["restless_ghost_altar"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
