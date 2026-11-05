"""Dev tool: run the maze solver on a fresh event, measuring its budget.

Clears the strange-box backlog, spawns the maze, then solves it and prints
how many ticks the run used. Staff only; never used by a bot script.

Usage: scripts/orchestrator.py run <account> probe_maze_solve
"""

from scripts import randomevents
from scripts.quests import solver


def run(bot, *args):
    bot.intend("probe: maze solve")
    solver.solve_strange_box(bot)
    bot.cheat("~maze")
    target = bot.world.ticks + 100
    while bot.world.ticks < target and not randomevents.in_maze(bot):
        bot.pump(2)
    if not randomevents.in_maze(bot):
        print("not in maze after cheat: %s" % (bot.position,), flush=True)
        return 1
    start = bot.world.ticks
    solved = randomevents.solve_maze(bot)
    print("maze solve=%s ticks=%d at %s" % (solved, bot.world.ticks - start, bot.position), flush=True)
    return 0 if solved else 1
