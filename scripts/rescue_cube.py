"""Rescue: clear the strange-box cubes sitting in the pack.

The quiz itself lives in the quest solver, which the quest runner already
uses; this is only the random-event entry point, so a botting script that
picks a box up is unblocked instead of tripping over the question window.

Usage: scripts/orchestrator.py run <account> rescue_cube
"""

import gamedata as knowledge

from scripts.quests.solver import solve_strange_box


def run(bot, *args):
    bot.intend("rescue: cube")
    solved = solve_strange_box(bot)
    cube = knowledge.objs.by_name("macro_cube")[0].obj_id
    left = bot.inventory().get(cube, 0)
    print("cube: %s, %d left" % ("solved" if solved else "unfinished", left), flush=True)
    return 0 if solved and left == 0 else 1
