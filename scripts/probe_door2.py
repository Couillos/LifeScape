"""Open the tutorial door step by step with full visibility.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_door2.py [username]
"""

import sys
import time


from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t11"
    bot = Bot()
    try:
        bot.login(username)
        bot.close_modal()
        bot.pump(5)
        print("start %s inv=%s" % (bot.position, bot.inventory()), flush=True)
        bot.walk_to(3097, 3107, open_doors=True)
        print("at door: %s loc=%s" % (bot.position, bot.world.loc_at(3098, 3107, 0)), flush=True)
        ok = bot.open_door(3098, 3107, timeout=30)
        print("open result: %s loc=%s" % (ok, bot.world.loc_at(3098, 3107, 0)), flush=True)
        bot.pump(10)
        print("after wait loc: %s pos: %s" % (bot.world.loc_at(3098, 3107, 0), bot.position), flush=True)
        try:
            bot.walk_to(3100, 3107, open_doors=True, timeout=60)
            print("through: %s" % (bot.position,), flush=True)
        except Exception as error:
            print("walk failed: %s at %s loc=%s" % (error, bot.position, bot.world.loc_at(3098, 3107, 0)), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
