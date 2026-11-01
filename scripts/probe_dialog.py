"""Talk to the nearest NPC and click through a linear dialog.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_dialog.py [username]
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t14"
    bot = Bot()
    try:
        bot.login(username)
        print("start %s" % (bot.position,), flush=True)
        bot.close_modal()
        bot.pump(5)
        print("modal after close: %s" % (bot.world.modal,), flush=True)
        deadline = time.time() + 30
        while not bot.world.npcs and time.time() < deadline:
            bot.pump(5)
        nid = min(bot.world.npcs, key=lambda i: abs(bot.world.npcs[i]["x"] - bot.world.x) + abs(bot.world.npcs[i]["z"] - bot.world.z))
        npc = bot.world.npcs[nid]
        print("talking to npc %d type %d at (%d,%d)" % (nid, npc["type"], npc["x"], npc["z"]), flush=True)
        bot.walk_to(npc["x"], npc["z"])
        bot.op_npc(1, nid)
        seen = set()
        deadline = time.time() + 60
        while time.time() < deadline:
            bot.pump(5)
            for com, text in sorted(bot.world.texts.items()):
                if (com, text) not in seen:
                    seen.add((com, text))
                    print("text [%d]: %r" % (com, text), flush=True)
            for who, text in bot.world.chat[-2:]:
                print("chat %s: %r" % (who, text), flush=True)
            bot.world.chat.clear()
            for text in bot.world.messages[-3:]:
                print("server: %r" % (text,), flush=True)
            bot.world.messages.clear()
            if bot.world.count_dialog:
                bot.resume_pcount(1)
                bot.world.count_dialog = False
            elif bot.world.modal:
                bot.resume_pausebutton()
            else:
                print("modal closed, done", flush=True)
                return
    finally:
        bot.close()


if __name__ == "__main__":
    main()
