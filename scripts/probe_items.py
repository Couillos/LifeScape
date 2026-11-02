"""Item loop probe: door, survival dialog, tab click, drop with full trace.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_items.py [username]
"""

import sys
import time


from lifescape.client import Bot


def drive(bot, answers, timeout=120):
    deadline = time.time() + timeout
    while time.time() < deadline:
        bot.pump(5)
        texts = dict(bot.chat_texts())
        options = [t for t in texts.values() if t in answers or "thank you" in t.lower() or "Nothing" in t]
        if options:
            bot.chat_choose(next((a for a in answers if a in options), options[0]))
        elif bot.world.modal:
            bot.chat_continue()
        else:
            return True
    return False


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t16"
    bot = Bot()
    try:
        bot.login(username)
        bot.close_modal()
        bot.pump(5)
        print("start %s inv=%s" % (bot.position, bot.inventory()), flush=True)
        bot.walk_to(3100, 3107, open_doors=True, timeout=120)
        print("through door: %s" % (bot.position,), flush=True)
        nid = bot.find_npc("survival")
        print("instructor:", nid, flush=True)
        if nid is not None:
            npc = bot.world.npcs[nid]
            bot.approach(npc["x"], npc["z"], open_doors=True)
            bot.talk_to(nid)
            drive(bot, ["No, thank you.", "Nothing, thanks."])
            print("after dialog inv:", bot.inventory(), flush=True)
            bot.tut_clickside(3)
            deadline = time.time() + 20
            while not bot.inventory() and time.time() < deadline:
                bot.pump(5)
            print("after tab: %s" % (bot.inventory(),), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
