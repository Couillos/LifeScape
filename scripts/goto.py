"""Dev tool: put an account on a known tile (staff teleport).

Only for staff accounts while testing: a bot stranded in scenery would
otherwise need a long walk the SDK may legitimately refuse. Never used by a
bot script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/goto.py <account> <x> <z> [level]
"""

import sys


from lifescape.client import Bot


def main():
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    account = sys.argv[1]
    x, z = int(sys.argv[2]), int(sys.argv[3])
    level = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    bot = Bot()
    try:
        bot.login(account, account)
        bot.intend("teleport to %d,%d,%d" % (x, z, level))
        bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
        target = bot.world.ticks + 100
        while bot.world.ticks < target:
            bot.pump(2)
            if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
                print("at %s" % (bot.position,), flush=True)
                return 0
        print("teleport did not land: %s" % (bot.position,), flush=True)
        return 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
