"""Requirement check and completion: Cook's Assistant."""

import sys


from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("cook")


def solve(bot):
    solver.talk_like(bot, "cook", ["what's wrong", "help you"])
    solver.talk(bot, "cook")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
