"""Climb down ladders until back on level 0.

Fail-teleport towers (old man/genie ignored) strand bots upstairs; this
walks them back to ground level so the skill bot can resume.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/rescue_down.py <username>
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from lifescape.client import Bot
from scripts.randomevents import climb_out


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "nina_evans"
    bot = Bot()
    try:
        bot.login(username, username)
        bot.pump(10)
        bot.intend("rescue: tower")
        print("start %s" % (bot.position,), flush=True)
        return 0 if climb_out(bot) else 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
