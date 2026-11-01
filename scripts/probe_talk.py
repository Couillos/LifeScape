"""Talk to the nearest NPC and show what the dialog looks like.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_talk.py [username]
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from lifescape.client import Bot


def nearest_npc(bot):
    best = None
    for nid, npc in bot.world.npcs.items():
        dist = abs(npc["x"] - bot.world.x) + abs(npc["z"] - bot.world.z)
        if best is None or dist < best[0]:
            best = (dist, nid)
    return best


def main():
    username = sys.argv[1] if len(sys.argv) > 1 else "t06"
    bot = Bot()
    try:
        bot.login(username)
        print("start %s" % (bot.position,), flush=True)
        deadline = time.time() + 30
        while not bot.world.npcs and time.time() < deadline:
            bot.pump(5)
        print("npcs: %s" % ({nid: (n["x"], n["z"]) for nid, n in bot.world.npcs.items()},), flush=True)
        dist, nid = nearest_npc(bot)
        npc = bot.world.npcs[nid]
        print("talking to npc %d at (%d,%d)" % (nid, npc["x"], npc["z"]), flush=True)
        bot.walk_to(npc["x"], npc["z"])
        npc = bot.world.npcs[nid]
        bot.op_npc(1, nid)
        for _ in range(6):
            for op, name, body in bot.pump(5):
                if name in ("IF_OPENCHAT", "IF_OPENMAIN", "IF_SETTEXT", "MESSAGE_GAME", "P_COUNTDIALOG"):
                    print("dialog %s (%d) %r" % (name, op, body), flush=True)
            print("modal=%s texts=%d chat=%s" % (bot.world.modal, len(bot.world.texts), bot.world.chat[-3:]), flush=True)
    finally:
        bot.close()


if __name__ == "__main__":
    main()
