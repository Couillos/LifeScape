"""Full tutorial start: talk to the guide, answer, walk out through the door.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_tutorial.py [username]
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape.client import Bot


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t15"
    bot = Bot()
    try:
        bot.login(username)
        print("start %s" % (bot.position,), flush=True)
        bot.close_modal()
        bot.pump(5)
        deadline = time.time() + 30
        while not bot.world.npcs and time.time() < deadline:
            bot.pump(5)
        nid = min(bot.world.npcs, key=lambda i: abs(bot.world.npcs[i]["x"] - bot.world.x) + abs(bot.world.npcs[i]["z"] - bot.world.z))
        bot.walk_to(bot.world.npcs[nid]["x"], bot.world.npcs[nid]["z"])
        bot.talk_to(nid)
        deadline = time.time() + 30
        while not bot.world.modal and time.time() < deadline:
            bot.pump(5)
        answered = False
        deadline = time.time() + 120
        while time.time() < deadline:
            bot.pump(5)
            if not answered and any("thank you" in text.lower() for _, text in bot.chat_texts()):
                bot.chat_choose("No, thank you.")
                answered = True
                print("answered no", flush=True)
            elif bot.world.modal:
                bot.chat_continue()
            else:
                break
        print("dialog done at %s" % (bot.position,), flush=True)
        bot.walk_to(3100, 3107, open_doors=True, timeout=180)
        print("through the door: %s" % (bot.position,), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
