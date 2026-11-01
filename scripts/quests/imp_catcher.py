"""Requirement check and completion: Imp Catcher."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("imp")


def solve(bot):
    solver.talk(bot, "wizard_mizgog", answers=["Give me a quest!", "Give me a quest please."])
    solver.talk(bot, "wizard_mizgog")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
