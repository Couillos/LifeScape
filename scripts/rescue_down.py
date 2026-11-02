"""Climb down ladders until back on level 0.

Fail-teleport towers (old man/genie ignored) strand bots upstairs; this
walks them back to ground level so the skill bot can resume.

Usage: scripts/orchestrator.py run <account> rescue_down
"""

from scripts.randomevents import climb_out


def run(bot, *args):
    bot.pump(10)
    bot.intend("rescue: tower")
    print("start %s" % (bot.position,), flush=True)
    return 0 if climb_out(bot) else 1
