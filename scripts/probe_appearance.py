"""Dump the bytes around a player-info appearance desync.

Diagnostic only. Run: ../lostcity/.pypy/bin/pypy3 scripts/probe_appearance.py <user>
"""
import sys
import time


from lifescape import world as world_mod
from lifescape.client import Bot

orig_read_appearance = world_mod.World._read_appearance


def traced(self, data, pid):
    start = data.pos
    try:
        return orig_read_appearance(self, data, pid)
    except IndexError:
        print("appearance overrun pid=%r start=%d len=%d" % (pid, start, len(data._data)), flush=True)
        print("bytes:", data._data[start:start + 120].hex(), flush=True)
        raise


world_mod.World._read_appearance = traced

username = sys.argv[1] if len(sys.argv) > 1 else "olivia_king"
seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 20
bot = Bot()
try:
    bot.login(username, username)
    print("login ok at", bot.position, flush=True)
    end = time.time() + seconds
    while time.time() < end:
        bot.pump(5)
finally:
    bot.close()
