"""Requirement check and completion: Romeo & Juliet."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("romeojuliet")


def solve(bot):
    solver.talk_like(bot, "romeo", ["looking for a quest", "quest", "help", "yes"])
    solver.talk_like(bot, "juliet", ["romeo", "message", "yes"])
    solver.talk_like(bot, "romeo", ["juliet", "message", "sad"])
    solver.talk_like(bot, "father_lawrence", ["romeo", "juliet", "help", "potion"])
    solver.talk_like(bot, "apothecary", ["cadava", "berries", "potion"])
    solver.talk_like(bot, "apothecary", ["cadava", "potion", "berries"])
    solver.talk_like(bot, "juliet", ["potion", "plan", "yes"])
    solver.talk_like(bot, "romeo", ["juliet", "plan", "crypt"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
