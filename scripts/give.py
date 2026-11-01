"""Dev tool: hand items to a staff account (testing only).

A quest requirement needs a bank item the test account does not have yet;
this puts it in the backpack through the staff cheat. Never used by a bot
script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/give.py <account> <item> [count]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata"))

from lifescape.client import Bot

import gamedata as knowledge


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    account = sys.argv[1]
    item = sys.argv[2]
    count = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    bot = Bot()
    try:
        bot.login(account, account)
        bot.intend("give %s x%d" % (item, count))
        bot.cheat("give %s %d" % (item, count))
        target = bot.world.ticks + 120
        obj_id = knowledge.objs.by_name(item)[0].obj_id
        while bot.world.ticks < target:
            bot.pump(2)
            if bot.inventory().get(obj_id, 0) >= count:
                print("have %s x%d" % (item, bot.inventory().get(obj_id, 0)), flush=True)
                return 0
        print("give failed: inventory %s" % (bot.inventory(),), flush=True)
        return 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
