"""Run the fishing bot with a player-info desync trace. Diagnostic only."""
import os
import runpy
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "gamedata"))

from lifescape import world as world_mod

orig_read_mask_set = world_mod.World._read_mask_set


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


world_mod.World._read_mask_set = traced

sys.argv = [os.path.join(os.path.dirname(__file__), "fishing", "run.py")] + sys.argv[1:]
runpy.run_path(os.path.join(os.path.dirname(__file__), "fishing", "run.py"), run_name="__main__")
