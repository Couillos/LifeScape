"""Measure wall-clock time for a depleted normal tree to respawn.

Proves game-time acceleration: normal trees respawn 72-120s game-time,
so ~12-20s wall at 100ms ticks vs 72-120s at 600ms.

Usage: scripts/orchestrator.py run <account> probe_respawn
"""

import time


from lifescape.client import Unreachable
from scripts.tutorial import common as tutorial_common


def run(bot, *args):
    bot.ensure_nav()
    bot.pump(10)
    print("start %s" % (bot.position,), flush=True)
    tree = tutorial_common.find_loc(bot, "tree")
    tree_x, tree_z, _level, tree_loc = tree
    print("tree at (%d,%d)" % (tree_x, tree_z), flush=True)
    bot.approach(tree_x, tree_z, open_doors=True)
    bot.op_loc(1, tree_x, tree_z, tree_loc)
    end = bot.world.ticks + 6000
    stump_at = None
    while bot.world.ticks < end:
        bot.pump(5)
        live = bot.world.locs.get((tree_x, tree_z, bot.world.level))
        static = bot.world.static_locs.get((tree_x, tree_z, bot.world.level))
        loc_id = (live or static or (None,))[0]
        name = bot.loctypes["loc"][loc_id]["name"] if loc_id else None
        if name != "tree" and stump_at is None:
            stump_at = time.time()
            print("depleted at tick %d (%s)" % (bot.world.ticks, name), flush=True)
        if stump_at is not None and name == "tree":
            print("respawned after %.0fs wall" % (time.time() - stump_at), flush=True)
            return 0
    print("no respawn within 120s", flush=True)
    return 1
