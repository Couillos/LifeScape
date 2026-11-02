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
    # completed stages never need re-talking. Stage-gate the rest too:
    # restarts otherwise walk every leg again.
    stage = _saved_stage()
    if stage < 1:
        solver.talk_like(bot, "duke_of_lumbridge", ["talisman", "help", "runes", "yes"])
    # Sedridor opens with "looking for the head wizard" (the "Nothing
    # thanks" first option is a dead end), then takes the talisman with
    # "here you are" and hands the package on "certainly".
    if stage < 3:
        solver.talk_like(bot, "head_wizard", ["looking for the head wizard", "here you are", "certainly", "yes"])
    if stage < 4:
        solver.talk_like(bot, "aubury", ["package", "research", "head_wizard", "notes"])
    if stage < 5:
        solver.talk_like(bot, "aubury", ["notes", "head_wizard", "finished"])
    elif bot.inventory().get(solver.obj_id("research_notes"), 0) == 0:
        # Notes lost to a full pack (unbankable junk has since been
        # dropped): Aubury re-issues them on re-talk.
        solver.talk_like(bot, "aubury", ["notes", "head_wizard", "finished"])
    # The finale is item-on-NPC, not talk: Sedridor only takes the notes
    # through opnpcu.
    solver.use_item_on_npc(bot, "research_notes", "head_wizard", near=(3103, 9571))
    solver.wait_complete(bot)


if __name__ == "__main__":
    sys.exit(run(QUEST, solve=solve))
