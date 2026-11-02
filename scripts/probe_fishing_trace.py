"""Run the fishing bot with a player-info desync trace. Diagnostic only.

Usage: scripts/orchestrator.py run <account> probe_fishing_trace [args...]
"""
import os
import runpy


from lifescape import world as world_mod

orig_read_mask_set = world_mod.World._read_mask_set

_installed = False


def traced(self, data, pid, player):
    start = data.pos
    header = data._data[start]
    try:
        return orig_read_mask_set(self, data, pid, player)
    except IndexError:
        print("mask desync pid=%r player=%s header=%#x pos=%d len=%d"
              % (pid, player, header, start, len(data._data)), flush=True)
        print("body:", data._data.hex(), flush=True)
        print("players:", {k: v.get("name") for k, v in self.players.items()}, flush=True)
        raise


def _install():
    global _installed
    if _installed:
        return
    world_mod.World._read_mask_set = traced
    _installed = True


def run(bot, *args):
    _install()
    namespace = runpy.run_path(os.path.join(os.path.dirname(__file__), "fishing", "run.py"))
    return namespace["run"](bot, *args)
