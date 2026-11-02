"""Dev tool: dump live quest varps after login (staff testing only).

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_vars.py <account>
"""

import sys


from lifescape.client import Bot

import gamedata as knowledge

WANTED = ("cookquest", "qp", "runemysteries", "demonstart", "sheep", "prieststart")


def main():
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    account = sys.argv[1]
    bot = Bot()
    try:
        bot.login(account, account)
        bot.pump(200)
        for name in WANTED:
            print("%s id=%s live=%s" % (name, knowledge.varp_id_by_name(name), bot.world.varps.get(knowledge.varp_id_by_name(name))), flush=True)
        print("live-nonzero=%s" % ({k: v for k, v in bot.world.varps.items() if v}), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
