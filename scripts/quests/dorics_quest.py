"""Requirement check and completion: Doric's Quest."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("doric")


def solve(bot):
    solver.talk(bot, "doric", answers=["I wanted to use your anvils.", "Yes, I will get you materials."])
    solver.talk(bot, "doric")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
