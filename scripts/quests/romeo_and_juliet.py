"""Requirement check and completion: Romeo & Juliet."""



from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

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


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
