"""Dev tool: drop N of an item on the ground (staff testing only).

Frees inventory space when the bank deposit flow is unusable. Never used
by a bot script.

Usage: scripts/orchestrator.py run <account> drop <item> [count]
"""

from lifescape.client import Unreachable
from lifescape.waits import wait_backpack

import gamedata as knowledge


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> drop <item> [count]", flush=True)
        return 2
    item = args[0]
    count = int(args[1]) if len(args) > 1 else 1
    found = knowledge.objs.by_name(item)
    if not found:
        print("unknown item %r" % (item,), flush=True)
        return 1
    obj_id = found[0].obj_id
    wait_backpack(bot, knowledge.com_id("inventory:inv"))
    dropped = 0
    while dropped < count:
        try:
            if not bot.drop(obj_id, timeout=750):
                break
        except Unreachable:
            break
        bot.pump(3)
        dropped += 1
    print("dropped %s x%d" % (item, dropped), flush=True)
    return 0
