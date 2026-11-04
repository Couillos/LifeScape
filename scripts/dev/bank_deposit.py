"""Dev tool: deposit the backpack at the nearest bank (staff testing only).

Frees inventory space before spawning test items with the give script.
Keeps the listed debugnames (default: coins). Never used by a bot script.

Usage: scripts/orchestrator.py run <account> bank_deposit [keep ...]
"""

import gamedata as knowledge
from scripts.tutorial import common as tutorial_common


def run(bot, *args):
    keep = set()
    for name in args:
        found = knowledge.objs.by_name(name)
        if found:
            keep.add(found[0].obj_id)
    booth = tutorial_common.find_loc(bot, "bankbooth")
    if booth is None:
        print("no bank booth nearby", flush=True)
        return 1
    booth_x, booth_z, _level, booth_loc = booth
    bot.open_bank(booth_x, booth_z, booth_loc)
    side_com = knowledge.com_id("bank_side:inv")
    if not bot.bank_deposit_all(side_com, keep_ids=keep):
        print("deposit did not clear the backpack", flush=True)
        for text in bot.world.messages[-5:]:
            print("game msg: %r" % (text[:100],), flush=True)
    _com, slots = bot.backpack()
    print("backpack now holds %d slots" % len(slots), flush=True)
    return 0
