"""One supervised account: its process, phase and reported state.

Phases and transitions (driven by daemon.supervise):

    bot      --crash-->  cleanup  --ok-->  restart -> bot
    bot      --event-->  (script exits, rescue set)
    bot      --crash with rescue-->  cleanup --> repair -> cleanup -> bot
    any      --stop-->   stopped

A ``cleanup`` phase runs ``clean_session``: log in and log out, which
frees a session left behind by a hard kill before the next attempt.
"""

import time

from .paths import read_json, user_paths


class Job:
    def __init__(self, account, script, args=()):
        self.account = account
        self.main_script = script
        self.script = script
        self.phase = "bot"  # bot | repair | cleanup
        self.args = list(args)
        self.process = None
        self.status = "starting"
        self.started = time.time()
        self.restarts = 0
        self.rescue = None
        self.restart_at = None
        self.log_offset = 0
        self.last_event = None
        self.last_requirements = None
        self.last_code = None
        self.supervise = True
        self.supervisor = None

    def alive(self):
        return self.process is not None and self.process.poll() is None

    def describe(self):
        """Merged daemon + bot view, written to run/users/<account>.json."""
        _merged, partial, log_path = user_paths(self.account)
        bot_state = read_json(partial, {}) or {}
        intention = bot_state.get("intention") or ""
        if self.status not in ("stopped", "finished", "failed"):
            if self.phase == "cleanup":
                intention = "session cleanup"
            elif self.phase == "repair":
                intention = "rescue " + self.script
        return {
            "account": self.account,
            "script": self.main_script,
            "args": self.args,
            "phase": self.phase,
            "status": self.status,
            "pid": self.process.pid if self.process is not None else None,
            "started": self.started,
            "restarts": self.restarts,
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
            "log": log_path,
            "last_event": self.last_event,
            "requirements": self.last_requirements,
            "last_code": self.last_code,
        }
