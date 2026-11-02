"""Talk to the guide and answer the first choice.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_guide.py [username] [yes|no]
"""

import sys
import time


from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t08"
    answer = sys.argv[2] if len(sys.argv) > 2 else "no"
    bot = Bot()
    try:
        bot.login(username)
        bot.close_modal()
        bot.pump(5)
        deadline = time.time() + 30
        while not bot.world.npcs and time.time() < deadline:
            bot.pump(5)
        nid = min(bot.world.npcs, key=lambda i: abs(bot.world.npcs[i]["x"] - bot.world.x) + abs(bot.world.npcs[i]["z"] - bot.world.z))
        npc = bot.world.npcs[nid]
        print("guide %d at (%d,%d)" % (nid, npc["x"], npc["z"]), flush=True)
        bot.walk_to(npc["x"], npc["z"])
        bot.talk_to(nid)
        seen = set()
        deadline = time.time() + 30
        while not bot.world.modal and time.time() < deadline:
            bot.pump(5)
        if not bot.world.modal:
            print("no dialog opened", flush=True)
            return
        deadline = time.time() + 90
        answered = False
        while time.time() < deadline:
            bot.pump(5)
            for com, text in bot.chat_texts():
                if (com, text) not in seen:
                    seen.add((com, text))
                    print("text [%d]: %r" % (com, text), flush=True)
            if not answered and any("thank you" in text.lower() for _, text in bot.chat_texts()):
                bot.chat_choose("Yes please." if answer == "yes" else "No, thank you.")
                answered = True
                print("answered %s" % answer, flush=True)
            elif bot.world.modal:
                bot.chat_continue()
            else:
                print("dialog over, modal=%s" % (bot.world.modal,), flush=True)
                return
        print("timed out", flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
