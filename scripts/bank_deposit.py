"""Dev tool: deposit the backpack at the nearest bank (staff testing only).

Frees inventory space before spawning test items with the give script.
Keeps the listed debugnames (default: coins). Never used by a bot script.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/bank_deposit.py <account> [keep ...]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata"))

from lifescape.client import Bot, Unreachable

import gamedata as knowledge
from scripts.tutorial import common as C


def main():
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    account = sys.argv[1]
    keep = set()
    for name in sys.argv[2:]:
        found = knowledge.objs.by_name(name)
        if found:
            keep.add(found[0].obj_id)
    bot = Bot()
    try:
        bot.login(account, account)
        booth = C.find_loc(bot, "bankbooth")
        if booth is None:
            print("no bank booth nearby", flush=True)
            return 1
        booth_x, booth_z, _level, booth_loc = booth
        bot.open_bank(booth_x, booth_z, booth_loc)
        side_com = knowledge.com_id("bank_side:inv")
        if not bot.bank_deposit_all(side_com, keep_ids=keep):
            print("deposit did not clear the backpack", flush=True)
            return 1
        _com, slots = bot.backpack()
        print("backpack now holds %d slots" % len(slots), flush=True)
        return 0
    except (Unreachable, TimeoutError) as error:
        print("bank deposit failed: %s" % (error,), flush=True)
        return 1
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
