"""Rescue a bot stuck in the Mysterious Old Man maze random event.

Usage: scripts/orchestrator.py run <account> rescue_maze
"""

from scripts.randomevents import in_maze, solve_maze


def run(bot, *args):
    bot.pump(10)
    bot.intend("rescue: maze")
    print("start %s" % (bot.position,), flush=True)
    if not in_maze(bot):
        print("not in maze, nothing to do", flush=True)
        return 0
    return 0 if solve_maze(bot) else 1
