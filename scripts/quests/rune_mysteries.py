"""Requirement check and completion: Rune Mysteries."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "gamedata"))

from gamedata import quest_requirements
from scripts.quests import solver
from scripts.quests.common import run

QUEST = quest_requirements.by_key("runemysteries")


def _saved_stage():
    """Save-based quest stage (live varps are blind to pre-login stages)."""
    account = os.environ.get("LIFESCAPE_USER") or (sys.argv[1] if len(sys.argv) > 1 else "")
    if not account:
        return 0
    try:
        from lifescape import save as save_mod
        varps = save_mod.load_account(account).get("varps", {})
        return varps.get(QUEST.completion_varps[0][0], 0) or 0
    except Exception:
        return 0


def solve(bot):
    # Duke re-talk is pure cost once started (castle interior doors);
    # completed stages never need re-talking.
    if _saved_stage() < 1:
        solver.talk_like(bot, "duke_of_lumbridge", ["talisman", "help", "runes", "yes"])
    solver.talk_like(bot, "head_wizard", ["talisman", "duke", "help", "yes"])
    solver.talk_like(bot, "head_wizard", ["package", "research", "aubury"])
    solver.talk_like(bot, "aubury", ["package", "research", "head_wizard", "notes"])
    solver.talk_like(bot, "aubury", ["notes", "head_wizard", "finished"])
    solver.talk_like(bot, "head_wizard", ["notes", "aubury", "research"])
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
