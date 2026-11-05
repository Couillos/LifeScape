"""Dump the bytes around a player-info appearance desync.

Diagnostic only.

Usage: scripts/orchestrator.py run <account> probe_appearance [seconds]
"""


from lifescape import world as world_mod

orig_read_appearance = world_mod.World._read_appearance


def traced(self, data, pid):
    start = data.pos
    try:
        return orig_read_appearance(self, data, pid)
    except IndexError:
        print("appearance overrun pid=%r start=%d len=%d" % (pid, start, len(data._data)), flush=True)
        print("bytes:", data._data[start:start + 120].hex(), flush=True)
        raise


def run(bot, *args):
    seconds = float(args[0]) if len(args) > 0 else 20
    world_mod.World._read_appearance = traced
    print("login ok at", bot.position, flush=True)
    end = bot.world.ticks + int(seconds * 50)
    while bot.world.ticks < end:
        bot.pump(5)
