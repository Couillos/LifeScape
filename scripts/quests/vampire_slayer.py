"""Requirement check and completion: Vampire Slayer."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("vampire")


def solve(bot):
    solver.talk_like(bot, "morgan", ["adventure", "yes", "help"])
    # Harlow wants a beer before he talks: one from the bank, bought once.
    if bot.inventory().get(solver.obj_id("beer"), 0) < 1:
        try:
            bot.bank_withdraw(solver.obj_id("beer"), 1)
        except Exception:
            pass
    solver.talk_like(bot, "dr_harlow", ["morgan", "help", "beer"])
    solver.talk_like(bot, "dr_harlow", ["beer", "ok mate", "here"])
    solver.operate_loc(bot, ("2614", "vampcoffin"), 1)
    solver.attack(bot, ("count_draynor",), timeout=600)
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
