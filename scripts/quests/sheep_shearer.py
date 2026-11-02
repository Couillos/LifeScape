"""Requirement check and completion: Sheep Shearer."""



from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

QUEST = quest_requirements.by_key("sheep")


def solve(bot):
    solver.talk(bot, "fred_the_farmer", answers=["I'm looking for a quest.", "Yes okay. I can do that."])
    solver.talk(bot, "fred_the_farmer")


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
