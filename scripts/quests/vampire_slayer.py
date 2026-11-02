"""Requirement check and completion: Vampire Slayer."""

import os
import sys


from gamedata import quest_requirements
from lifescape import save as save_mod
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("vampire")


def _saved_stage():
    """Save-based quest stage (live varps are blind to pre-login stages)."""
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return 0
    try:
        varps = save_mod.load_account(account).get("varps", {})
        return varps.get(QUEST.completion_varps[0][0], 0) or 0
    except Exception:
        return 0


def solve(bot):
    # Morgan's house traps routing (one-way door, lantern maze): never
    # walk back for a re-talk once started.
    if _saved_stage() < 1:
        solver.talk_like(bot, "morgan", ["adventure", "yes", "help"])
    # Harlow wants a beer before he talks: one from the bank, bought once.
    if bot.inventory().get(solver.obj_id("beer"), 0) < 1:
        try:
            bot.bank_withdraw(solver.obj_id("beer"), 1)
        except Exception:
            pass
    if _saved_stage() < 2:
        solver.talk_like(bot, "dr_harlow", ["morgan", "help", "beer"])
        solver.talk_like(bot, "dr_harlow", ["beer", "ok mate", "here"])
    solver.operate_loc(bot, ("2614", "vampcoffin"), 1)
    solver.attack(bot, ("count_draynor",), timeout=600)
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
