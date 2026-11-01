"""Dev tool: probe which obj debugnames the server cheat accepts (staff only).

Gives each named item once and reports the inventory delta. Never used by
a bot script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_give.py <account> <item[=count]>...
Gives count of each item (default 1) and reports the inventory delta.
Never used by a bot script.
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata"))

from lifescape.client import Bot

import gamedata as knowledge


def parse(spec):
    if "=" in spec:
        name, count = spec.split("=", 1)
        return name, max(1, int(count))
    return spec, 1


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    account, specs = sys.argv[1], sys.argv[2:]
    bot = Bot()
    try:
        bot.login(account, account)
        for spec in specs:
            name, count = parse(spec)
            found = knowledge.objs.by_name(name)
            if not found:
                print("%s: UNKNOWN-LOCALLY" % name, flush=True)
                continue
            obj_id = found[0].obj_id
            before = bot.inventory().get(obj_id, 0)
            bot.cheat("give %s %d" % (name, count))
            target = time.time() + 8
            while time.time() < target:
                bot.pump(2)
                if bot.inventory().get(obj_id, 0) >= before + count:
                    break
            after = bot.inventory().get(obj_id, 0)
            print("%s: %d -> %d %s" % (name, before, after, "OK" if after >= before + count else "REFUSED"), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
