"""Requirement check and completion: The Restless Ghost."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("priest")


def solve(bot):
    solver.talk_like(bot, "father_aereck", ["looking for a quest", "quest"])
    solver.talk_like(bot, "father_urhney", ["aereck", "ghost", "sent me"])
    # The generic "ghost" also exists on other maps: stay in the graveyard.
    solver.talk_like(bot, "ghost", ["problem", "doomed", "skull", "yep", "yes"], near=(3249, 3194))
    solver.operate_loc(bot, ["shutghostcoffin"], op=1)
    solver.take_near(bot, "ghostskull")
    solver.use_item_on_loc(bot, "ghostskull", ["restless_ghost_altar"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
