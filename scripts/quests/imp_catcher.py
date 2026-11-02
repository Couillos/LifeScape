"""Requirement check and completion: Imp Catcher."""

import sys


from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("imp")


def solve(bot):
    solver.talk(bot, "wizard_mizgog", answers=["Give me a quest!", "Give me a quest please."])
    solver.talk(bot, "wizard_mizgog")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
