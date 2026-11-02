"""Dev probe: one paced bank deposit with server-rejection dump (staff only).

Diagnoses bank_side:inv op4 bursts that land zero slots. Never used by bots.
"""

import sys

from lifescape.client import Bot

import gamedata as knowledge
from scripts.tutorial import common as tutorial_common


def main():
    account = sys.argv[1] if len(sys.argv) > 1 else "admin"
    bot = Bot()
    try:
        bot.login(account, account)
        bot.pump(10)
        booth = tutorial_common.find_loc(bot, "bankbooth")
        print("booth %s" % (booth,), flush=True)
        if len(sys.argv) > 2 and sys.argv[2] == "nobank":
            print("stopping before open_bank", flush=True)
            return 0
        booth_x, booth_z, booth_level, booth_loc = booth
        bot.open_bank(booth_x, booth_z, booth_loc)
        print("bank open, modal=%s" % (bot.world.modal,), flush=True)
        side_com = knowledge.com_id("bank_side:inv")
        _com, slots = bot.backpack()
        print("backpack slots=%d side_com=%s" % (len(slots), side_com), flush=True)
        target = sorted(slots)[0]
        held, _count = slots[target]
        print("depositing slot=%d obj=%d" % (target, held), flush=True)
        bot.inv_button(4, held, target, side_com)
        seen_msgs = len(bot.world.messages)
        for round_no in range(12):
            unsets = sum(1 for _op, name, _body in bot.pump(5) if name == "UNSET_MAP_FLAG")
            for text in bot.world.messages[seen_msgs:]:
                print("game msg: %r" % (text[:100],), flush=True)
            seen_msgs = len(bot.world.messages)
            _com, slots = bot.backpack()
            print("round %d modal=%s slots=%d unsets=%d" % (round_no, bot.world.modal, len(slots), unsets), flush=True)
        return 0
    finally:
        bot.close()


if __name__ == "__main__":
    sys.exit(main())
