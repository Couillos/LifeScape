"""Dev tool: click a loc, then use an item on it, dumping modals (staff testing).

Shows whether the server runs the script (mesbox text) or drops the click.
Never used by bots.

Usage: scripts/orchestrator.py run <account> probe_use <loc> [item] [op] [standx] [standz]
Stand tile forces the click position instead of the nearest approach.
"""

from scripts.quests import solver
from scripts.tutorial import common as tutorial_common

import gamedata.objs


def dump(bot, tag):
    print("--- %s modal=%s pos=%s" % (tag, bot.world.modal, bot.position), flush=True)
    for com, text in bot.chat_texts()[-6:]:
        print("  text %s: %r" % (com, text[:100]), flush=True)
    for com, text in bot.chat_options():
        print("  option %s: %r" % (com, text[:100]), flush=True)
    for (loc_x, loc_z, loc_level), entry in bot.world.locs.items():
        if loc_level == bot.world.level and abs(loc_x - bot.world.x) + abs(loc_z - bot.world.z) <= 2 and entry is not None:
            print("  near loc id=%d at %d,%d" % (entry[0], loc_x, loc_z), flush=True)


def run(bot, *args):
    if len(args) < 1:
        print("usage: run <account> probe_use <loc> [item] [op] [standx] [standz]", flush=True)
        return 2
    loc_name = args[0]
    item_name = args[1] if len(args) > 1 else None
    op = int(args[2]) if len(args) > 2 else 1
    stand = (int(args[3]), int(args[4])) if len(args) > 4 else None
    bot.pump(10)
    x, z, level, loc = tutorial_common.find_loc(bot, loc_name)
    print("loc %s at %d,%d,%d id=%d" % (loc_name, x, z, level, loc), flush=True)
    if stand is not None:
        bot.walk_to(stand[0], stand[1], level, timeout=6000, open_doors=True)
    else:
        try:
            bot.approach(x, z, level=level, radius=2, timeout=6000, open_doors=True)
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
        tutorial_common.use_held_on_loc(bot, x, z, loc, solver.obj_id(item_name))
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
