"""Requirement check and completion: Sheep Shearer."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("sheep")


def solve(bot):
    solver.talk(bot, "fred_the_farmer", answers=["I'm looking for a quest.", "Yes okay. I can do that."])
    solver.talk(bot, "fred_the_farmer")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
