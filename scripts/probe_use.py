"""Dev tool: click a loc, then use an item on it, dumping modals (staff testing).

Shows whether the server runs the script (mesbox text) or drops the click.
Never used by bots.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_use.py <account> <loc> [item] [op] [standx] [standz]
Stand tile forces the click position instead of the nearest approach.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata"))

from lifescape.client import Bot
from scripts.tutorial import common as C

import gamedata.objs


def dump(bot, tag):
    print("--- %s modal=%s pos=%s" % (tag, bot.world.modal, bot.position), flush=True)
    for com, text in bot.chat_texts()[-6:]:
        print("  text %s: %r" % (com, text[:100]), flush=True)
    for com, text in bot.chat_options():
        print("  option %s: %r" % (com, text[:100]), flush=True)


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    account, loc_name = sys.argv[1], sys.argv[2]
    item_name = sys.argv[3] if len(sys.argv) > 3 else None
    op = int(sys.argv[4]) if len(sys.argv) > 4 else 1
    stand = (int(sys.argv[5]), int(sys.argv[6])) if len(sys.argv) > 6 else None
    bot = Bot()
    try:
        bot.login(account, account)
        bot.pump(10)
        x, z, level, loc = C.find_loc(bot, loc_name)
        print("loc %s at %d,%d,%d id=%d" % (loc_name, x, z, level, loc), flush=True)
        if stand is not None:
            bot.walk_to(stand[0], stand[1], level, timeout=120, open_doors=True)
        else:
            try:
                bot.approach(x, z, level=level, radius=2, timeout=120, open_doors=True)
            except Exception as error:
                print("approach failed: %s" % (error,), flush=True)
        print("at %s" % (bot.position,), flush=True)
        seen_msgs = len(bot.world.messages)
        bot.op_loc(op, x, z, loc)
        for round_no in range(6):
            unsets = sum(1 for _op, name, _body in bot.pump(5) if name == "UNSET_MAP_FLAG")
            for text in bot.world.messages[seen_msgs:]:
                print("  game msg: %r" % (text[:100],), flush=True)
            seen_msgs = len(bot.world.messages)
            dump(bot, "op%d round %d unsets=%d" % (op, round_no, unsets))
            if bot.world.modal is None:
                break
        if item_name is not None:
            from scripts.quests import solver
            C.use_held_on_loc(bot, x, z, loc, solver.obj_id(item_name))
            print("sent use %s on %s" % (item_name, loc_name), flush=True)
            seen_msgs = len(bot.world.messages)
            for round_no in range(10):
                unsets = sum(1 for _op, name, _body in bot.pump(5) if name == "UNSET_MAP_FLAG")
                for text in bot.world.messages[seen_msgs:]:
                    print("  game msg: %r" % (text[:100],), flush=True)
                seen_msgs = len(bot.world.messages)
                dump(bot, "use round %d unsets=%d" % (round_no, unsets))
                if bot.world.modal is None and round_no >= 2:
                    break
            print("%s now: %d" % (item_name, bot.inventory().get(solver.obj_id(item_name), 0)), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
