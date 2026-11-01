"""Dev tool: probe one door instance (staff testing only).

Teleports next to the tile, stands on a server-accepted side, clicks it
once, then tries to step onto it. Prints every intermediate result so a
refused click or a refused step is unambiguous.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_door.py <account> <x> <z> [level]
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from lifescape import doors as doors_mod
from lifescape.client import Bot


def main():
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    account, x, z = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    level = int(sys.argv[4]) if len(sys.argv) > 4 else 0
    bot = Bot()
    try:
        bot.login(account, account)
        bot._ensure_nav()
        spot = bot._door_spot(x, z, level)
        print("spot=%s sides=%s" % (
            spot,
            bot._door_standing_sides(spot) if spot is not None else None,
        ), flush=True)
        loc = bot.world.loc_at(x, z, level)
        print("loc=%s" % (loc,), flush=True)
        if spot is None:
            print("no known door at %d,%d,%d" % (x, z, level), flush=True)
            return 1
        op = doors_mod.open_op(bot._loctypes, spot[9])
        print("op=%s" % (op,), flush=True)
        bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
        deadline = time.time() + 10
        while time.time() < deadline:
            bot.pump(2)
            if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
                break
        print("at=%s" % (bot.position,), flush=True)
        if not bot._stand_on_door_side(spot, timeout=15):
            print("no reachable side", flush=True)
            return 1
        print("side=%s" % ((bot.world.x, bot.world.z),), flush=True)
        if len(sys.argv) < 6 or sys.argv[5] != "noclick":
            if bot._try_step_on(x, z, level, time.time() + 3.0):
                print("step-first: OPEN ALREADY", flush=True)
                return 0
        before = bot.world.loc_at(x, z, level)
        if bot._open_door_inner(x, z, 10.0):
            print("PROBE OK: opened/stepped (was=%s now=%s)" % (before, bot.world.loc_at(x, z, level)), flush=True)
            return 0
        print("PROBE FAIL: still closed after click, loc=%s" % (bot.world.loc_at(x, z, level),), flush=True)
        return 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
