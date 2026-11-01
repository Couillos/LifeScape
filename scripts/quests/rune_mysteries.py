"""Requirement check and completion: Rune Mysteries."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("runemysteries")


def solve(bot):
    solver.talk_like(bot, "duke_of_lumbridge", ["talisman", "help", "runes", "yes"])
    solver.talk_like(bot, "head_wizard", ["talisman", "duke", "help", "yes"])
    solver.talk_like(bot, "head_wizard", ["package", "research", "aubury"])
    solver.talk_like(bot, "aubury", ["package", "research", "head_wizard", "notes"])
    solver.talk_like(bot, "aubury", ["notes", "head_wizard", "finished"])
    solver.talk_like(bot, "head_wizard", ["notes", "aubury", "research"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
