"""Dev tool: reset every quest on a staff account (testing only).

Runs the server's ~resetquests debugproc, which zeroes all quest stage
varps. Never used by a bot script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/reset_quests.py <account>
"""

import sys
import time


from lifescape.client import Bot


def main():
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    account = sys.argv[1]
    bot = Bot()
    try:
        bot.login(account, account)
        bot.intend("resetting quests")
        bot.cheat("~resetquests")
        deadline = time.time() + 20
        while time.time() < deadline:
            bot.pump(2)
        print("reset sent, at %s" % (bot.position,), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
