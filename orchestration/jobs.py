"""One supervised account: its session process, script and reported state.

A job is a persistent session (``open`` spawns the worker, ``close`` ends
it). Scripts come and go inside it (``run``/``unload``) without touching
the connection. The worker owns the session truth; the job mirrors it for
the CLI and the dashboard, and carries the rescue bookkeeping (which
script an unresolved random event interrupted).

Phases: ``idle`` (connected, no script) or ``running`` (a script thread
is active in the worker). Statuses: ``starting``, ``idle``, ``running``,
``busy`` (unload in progress), ``reconnecting``, ``finished``,
``failed``, ``disconnected``, ``stopped``.
"""

import threading
import time

from .paths import read_json, user_paths


class Job:
    def __init__(self, account, script=None, args=()):
        self.account = account
        self.script = script
        self.script_args = list(args)
        self.args = list(self.script_args)
        self.phase = "running" if script else "idle"
        self.process = None
        self.status = "starting"
        self.started = time.time()
        self.log_offset = 0
        self.last_error = None
        self.last_event = None
        self.last_requirements = None
        self.last_code = None
        self.interrupted = None  # (script, args) paused for a rescue
        self.rescue_for = None  # script currently running as a rescue
        self._worker_rescue_at = None  # worker last_event already rescued
        self.supervise = True
        self.supervisor = None
        self._guard = threading.RLock()

    def guard(self):
        return self._guard

    def alive(self):
        process = self.process
        return process is not None and process.poll() is None

    def describe(self):
        """Merged daemon + bot view, written to run/users/<account>.json."""
        _merged, partial, log_path = user_paths(self.account)
        bot_state = read_json(partial, {}) or {}
        intention = bot_state.get("intention") or ""
        if self.phase == "running" and self.script in ("rescue_maze", "rescue_down", "rescue_fallback"):
            intention = "rescue " + (self.script or "")
        entry = {
            "account": self.account,
            "script": self.script,
            "script_args": list(self.args),
            "args": list(self.args),
            "phase": self.phase,
            "status": self.status,
            "session_pid": self.process.pid if self.process is not None else None,
            "started": self.started,
            "intention": intention,
            "position": bot_state.get("position"),
            "hitpoints": bot_state.get("hitpoints"),
            "ticks": bot_state.get("ticks"),
            "updated": bot_state.get("updated"),
            "combat": bot_state.get("combat"),
            "energy": bot_state.get("energy"),
            "modal": bot_state.get("modal"),
            "hint": bot_state.get("hint"),
            "hint_type": bot_state.get("hint_type"),
            "flash": bot_state.get("flash"),
            "route": bot_state.get("route") or [],
            "route_target": bot_state.get("route_target"),
            "skills": bot_state.get("skills") or [],
            "worn": bot_state.get("worn") or [],
            "npcs": bot_state.get("npcs") or [],
            "locs": bot_state.get("locs") or [],
            "objs": bot_state.get("objs") or [],
            "players": bot_state.get("players") or [],
            "inv": bot_state.get("inv") or [],
            "bank": bot_state.get("bank") or [],
            "chat": bot_state.get("chat") or [],
            "messages": bot_state.get("messages") or [],
            "texts": bot_state.get("texts") or {},
            "log": log_path,
            "last_event": self.last_event,
            "requirements": self.last_requirements,
            "last_error": self.last_error,
            "last_code": self.last_code,
        }
        return entry
