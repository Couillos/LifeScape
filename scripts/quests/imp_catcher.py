"""Requirement check and completion: Imp Catcher."""



from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run as run_requirements

QUEST = quest_requirements.by_key("imp")


def solve(bot):
    solver.talk(bot, "wizard_mizgog", answers=["Give me a quest!", "Give me a quest please."])
    solver.talk(bot, "wizard_mizgog")


def run(bot, *args):
    return run_requirements(bot, QUEST, solve=solve)
