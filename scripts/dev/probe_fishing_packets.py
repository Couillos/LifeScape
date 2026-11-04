"""Run the fishing bot and print the packet that breaks player-info parsing.

Diagnostic only.

Usage: scripts/orchestrator.py run <account> probe_fishing_packets [args...]
"""
import collections
import os
import runpy


from lifescape import connection as connection_mod
from lifescape import world as world_mod

recent = collections.deque(maxlen=8)
orig_recv = connection_mod.Connection.recv
orig_read_appearance = world_mod.World._read_appearance

_installed = False


def recv(self):
    opcode, name, body = orig_recv(self)
    recent.append((name, len(body), body[:16].hex()))
    return opcode, name, body


def traced(self, data, pid):
    start = data.pos
    try:
        return orig_read_appearance(self, data, pid)
    except IndexError:
        print("appearance overrun pid=%r start=%d len=%d" % (pid, start, len(data._data)), flush=True)
        print("bytes:", data._data[start:start + 120].hex(), flush=True)
        print("recent packets:", list(recent), flush=True)
        raise


def _install():
    global _installed
    if _installed:
        return
    connection_mod.Connection.recv = recv
    world_mod.World._read_appearance = traced
    _installed = True


def run(bot, *args):
    _install()
    namespace = runpy.run_path(os.path.join(os.path.dirname(__file__), "fishing", "run.py"))
    return namespace["run"](bot, *args)
