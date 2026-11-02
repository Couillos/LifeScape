"""Survival instructor chain: talk, follow dialogs, watch for items/inv.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_survival.py [username]
"""

import sys
import time


from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t15"
    bot = Bot()
    try:
        bot.login(username)
        bot.close_modal()
        bot.pump(5)
        print("start %s inv=%s" % (bot.position, bot.inventory()), flush=True)
        deadline = time.time() + 30
        while not bot.world.npcs and time.time() < deadline:
            bot.pump(5)
        nid = bot.find_npc("survival")
        if nid is None:
            print("no survival instructor nearby: %s" % {i: (n["x"], n["z"]) for i, n in bot.world.npcs.items()}, flush=True)
            return
        npc = bot.world.npcs[nid]
        print("instructor %d at (%d,%d)" % (nid, npc["x"], npc["z"]), flush=True)
        bot.walk_to(npc["x"], npc["z"], open_doors=True)
        bot.talk_to(nid)
        deadline = time.time() + 60
        while not bot.world.modal and time.time() < deadline:
            bot.pump(5)
        seen = set()
        deadline = time.time() + 180
        while time.time() < deadline:
            bot.pump(5)
            for com, text in bot.chat_texts():
                if (com, text) not in seen:
                    seen.add((com, text))
                    print("text [%d]: %r" % (com, text), flush=True)
            options = [text for _, text in bot.chat_texts() if text in ("Yes please.", "No, thank you.")]
            answered = False
            for com, text in bot.chat_texts():
                if text in ("Yes please.", "No, thank you.", "I'll try.", "No thanks."):
                    bot.chat_choose(text)
                    print("chose %r" % text, flush=True)
                    answered = True
                    break
            if not answered and bot.world.modal:
                bot.chat_continue()
            elif not bot.world.modal and answered:
                pass
            print("modal=%s inv=%s" % (bot.world.modal, bot.inventory()), flush=True)
            if not bot.world.modal and len(seen) > 3:
                break
        print("done at %s inv=%s" % (bot.position, bot.inventory()), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
