"""Requirement check and completion: Goblin Diplomacy."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("gobdip")


def solve(bot):
    solver.talk(bot, "rustyanchor_bartender", answers=["Not very busy in here today, is it?"])
    solver.talk(bot, "general_wartface", answers=["Do you want me to pick an armour colour for you?"])
    solver.talk(bot, "general_wartface")
    solver.talk(bot, "general_wartface")
    solver.talk(bot, "general_wartface")


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
