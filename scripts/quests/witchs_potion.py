"""Requirement check and completion: Witch's Potion."""

import sys


from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("hetty")


def solve(bot):
    solver.talk(bot, "hetty", answers=["I am in search of a quest.", "Yes help me become one with my darker side."])
    solver.talk(bot, "hetty")
    solver.operate_loc(bot, ("hettycauldron",), 1)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
