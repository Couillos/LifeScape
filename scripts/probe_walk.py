"""Walk the bot and prove arrival by its own position updates.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_walk.py [username] [dx] [dz]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t03"
    dx = int(sys.argv[2]) if len(sys.argv) > 2 else 10
    dz = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    bot = Bot()
    try:
        bot.login(username)
        start = bot.position
        print("start %s" % (start,))
        target = (start[1] + dx, start[2] + dz)
        bot.walk_to(*target)
        print("arrived %s (target %s)" % (bot.position, target))
    finally:
        bot.close()


if __name__ == "__main__":
    main()
