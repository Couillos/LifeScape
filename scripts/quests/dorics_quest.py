"""Requirement check and completion: Doric's Quest."""

import sys


from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("doric")


def solve(bot):
    solver.talk(bot, "doric", answers=["I wanted to use your anvils.", "Yes, I will get you materials."])
    solver.talk(bot, "doric")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
