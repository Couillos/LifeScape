"""Dev tool: drop N of an item on the ground (staff testing only).

Frees inventory space when the bank deposit flow is unusable. Never used
by a bot script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/drop.py <account> <item> [count]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata"))

from lifescape.client import Bot, Unreachable
from lifescape.waits import wait_backpack

import gamedata as knowledge


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    account, item = sys.argv[1], sys.argv[2]
    count = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    bot = Bot()
    try:
        bot.login(account, account)
        found = knowledge.objs.by_name(item)
        if not found:
            print("unknown item %r" % (item,), flush=True)
            return 1
        obj_id = found[0].obj_id
        wait_backpack(knowledge.com_id("inventory:inv"))
        dropped = 0
        while dropped < count:
            try:
                if not bot.drop(obj_id, timeout=15):
                    break
            except Unreachable:
                break
            bot.pump(3)
            dropped += 1
        print("dropped %s x%d" % (item, dropped), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
