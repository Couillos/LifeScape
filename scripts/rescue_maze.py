"""Rescue a bot stuck in the Mysterious Old Man maze random event.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/rescue_maze.py <username>
"""

import sys


from lifescape.client import Bot
from scripts.randomevents import in_maze, solve_maze


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t48"
    bot = Bot()
    try:
        bot.login(username, username)
        bot.pump(10)
        bot.intend("rescue: maze")
        print("start %s" % (bot.position,), flush=True)
        if not in_maze(bot):
            print("not in maze, nothing to do", flush=True)
            return 0
        return 0 if solve_maze(bot) else 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
