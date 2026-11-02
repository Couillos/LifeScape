"""Requirement check and completion: Doric's Quest."""



from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

QUEST = quest_requirements.by_key("doric")


def solve(bot):
    solver.talk(bot, "doric", answers=["I wanted to use your anvils.", "Yes, I will get you materials."])
    solver.talk(bot, "doric")


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
