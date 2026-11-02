"""Requirement check and completion: Cook's Assistant."""



from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

QUEST = quest_requirements.by_key("cook")


def solve(bot):
    solver.talk_like(bot, "cook", ["what's wrong", "help you"])
    solver.talk(bot, "cook")


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
