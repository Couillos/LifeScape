"""Supervisor daemon: one worker process per account, one state file each.

It owns process lifetime (start/stop/restart), the log tail that carries
``@@EVENT`` markers from ``scripts/randomevents.py``, the rescue/cleanup
state machine, and the unix socket the CLI talks to. It never speaks the
game protocol: that stays in the SDK, inside the worker.
"""

import json
import os
import socket
import subprocess
import sys
import threading
import time

from . import holdings, naming, quest_state, registry, web
from .jobs import Job
from .paths import (
    PID_PATH,
    ROOT,
    SOCKET_PATH,
    USERS_DIR,
    ensure_dirs,
    read_json,
    user_paths,
    write_json,
)

MAX_RESTARTS = 30
RESTART_DELAY = 2.0
STOP_GRACE = 8.0
ONLINE_WINDOW = 15.0


def known_accounts():
    """Accounts the daemon has written state for, running or not."""
    try:
        names = os.listdir(USERS_DIR)
    except OSError:
        return []
    return [
        name[:-5] for name in names
        if name.endswith(".json") and not name.endswith(".bot.json")
    ]


class Orchestrator:
    def __init__(self):
        self.jobs = {}
        self.lock = threading.Lock()
        self.running = True
        ensure_dirs()

    # -- process control -------------------------------------------------

    def spawn(self, job, script):
        job.script = script
        # Keep the previous snapshot until the new worker writes its own:
        # the dashboard stays populated while a bot is restarting.
        with open(user_paths(job.account)[2], "ab") as handle:
            handle.write(("=== %s %s %s ===\n" % (job.account, script, time.strftime("%H:%M:%S"))).encode())
        job.process = subprocess.Popen(
            [sys.executable, "-m", "orchestration.worker", job.account, script] + job.args,
            cwd=ROOT,
            stdout=open(user_paths(job.account)[2], "ab"),
            stderr=subprocess.STDOUT,
            env=dict(
                os.environ,
                PYTHONPATH=ROOT,
                LIFESCAPE_USER=job.account,
                LIFESCAPE_SCRIPT=script,
                LIFESCAPE_STATE=user_paths(job.account)[1],
            ),
            start_new_session=True,
        )
        job.status = "running"
        job.started = time.time()
        print("job %s: %s pid=%d" % (job.account, script, job.process.pid), flush=True)

    def start_job(self, account, script, args):
        if not account or not script:
            return {"ok": False, "error": "account and script are required"}
        try:
            registry.script_path(script)
        except KeyError as error:
            return {"ok": False, "error": str(error)}
        with self.lock:
            existing = self.jobs.get(account)
            if existing is not None and existing.alive():
                return {"ok": False, "error": "%s already runs %s" % (account, existing.script)}
            job = Job(account, script, args)
            self.jobs[account] = job
            self.spawn(job, script)
            self.write_state(job)
        self.start_supervisor(job)
        return {"ok": True, "job": job.describe()}

    def start_supervisor(self, job):
        """One thread per account: accounts are watched in parallel."""
        job.supervisor = threading.Thread(target=self.supervise_job, args=(job,), daemon=True)
        job.supervisor.start()

    def stop_job(self, account, kill_after=STOP_GRACE):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no job for %s" % account}
        job.status = "stopping"
        job.restart_at = None
        job.supervise = False
        if job.alive():
            try:
                job.process.terminate()
            except OSError:
                pass
            deadline = time.time() + kill_after
            while time.time() < deadline and job.process.poll() is None:
                time.sleep(0.2)
            if job.process.poll() is None:
                job.process.kill()
        job.status = "stopped"
        job.supervise = False
        self.write_state(job)
        return {"ok": True, "job": job.describe()}

    # -- supervision -----------------------------------------------------

    def supervise_job(self, job):
        while self.running and job.supervise:
            try:
                self.supervise_once(job)
            except Exception as error:
                print("supervise error on %s: %r" % (job.account, error), flush=True)
            time.sleep(0.5)

    def supervise_once(self, job):
        self.read_events(job)
        if job.process is None:
            if job.restart_at is not None and time.time() >= job.restart_at:
                job.restart_at = None
                job.phase = "bot"
                self.spawn(job, job.main_script)
            self.write_state(job)
            return
        code = job.process.poll()
        if code is not None:
            self.handle_exit(job, code)
        self.write_state(job)

    def read_events(self, job):
        _merged, _partial, log_path = user_paths(job.account)
        try:
            with open(log_path, "rb") as handle:
                handle.seek(job.log_offset)
                chunk = handle.read()
                job.log_offset = handle.tell()
        except OSError:
            return
        for raw in chunk.splitlines():
            line = raw.decode("utf-8", "replace")
            if not line.startswith("@@EVENT "):
                continue
            try:
                event = json.loads(line[len("@@EVENT "):])
            except ValueError:
                continue
            self.handle_event(job, event)

    def handle_event(self, job, event):
        if event.get("kind") == "quest_requirements":
            completed = quest_state.completion(job.account, event.get("quest") or "")
            if completed is not None:
                event["completed"] = completed
            met = bool(event.get("met"))
            job.last_requirements = event
            print("job %s: quest %s requirements met=%s completed=%s (%d reasons)" % (
                job.account, event.get("quest"), met, event.get("completed"), len(event.get("reasons") or [])
            ), flush=True)
            return
        if event.get("kind") != "random_event":
            return
        name = event.get("event") or "unknown"
        resolved = bool(event.get("resolved"))
        print("job %s: random event %s resolved=%s" % (job.account, name, resolved), flush=True)
        job.last_event = {"event": name, "resolved": resolved, "at": time.time()}
        if resolved:
            return
        if name in registry.RESCUES:
            job.rescue = registry.RESCUES[name]
        else:
            job.status = "needs_human"

    def handle_exit(self, job, code):
        job.process = None
        if job.status in ("stopping", "stopped"):
            job.status = "stopped"
            return
        if job.status == "needs_human":
            return
        if job.main_script == "tests":
            # Not an account: no session to free, no relaunch.
            job.status = "finished" if code == 0 else "failed"
            job.last_code = code
            job.supervise = False
            return
        if job.phase == "cleanup":
            self.after_cleanup(job)
            return
        if job.phase == "repair":
            self.start_cleanup(job)
            return
        if code == 0:
            job.status = "finished"
            job.supervise = False
            return
        job.status = "crashed"
        job.last_code = code
        self.start_cleanup(job)

    def after_cleanup(self, job):
        job.phase = "bot"
        if job.rescue is not None:
            rescue = job.rescue
            job.rescue = None
            job.phase = "repair"
            job.status = "repairing"
            self.spawn(job, rescue)
            return
        if job.restarts >= MAX_RESTARTS:
            job.status = "failed"
            job.supervise = False
            return
        job.restarts += 1
        job.status = "restarting"
        job.restart_at = time.time() + RESTART_DELAY

    def start_cleanup(self, job):
        """Free the session (login+close) before the next attempt."""
        job.phase = "cleanup"
        job.status = "cleaning"
        self.spawn(job, "clean_session")

    def write_state(self, job):
        merged_path, _partial, _log = user_paths(job.account)
        write_json(merged_path, job.describe())

    def restore_jobs(self):
        """Relaunch jobs that were running when the daemon last stopped."""
        try:
            names = sorted(os.listdir(USERS_DIR))
        except OSError:
            return
        for name in names:
            if not name.endswith(".json") or name.endswith(".bot.json"):
                continue
            state = read_json(os.path.join(USERS_DIR, name), {}) or {}
            account = state.get("account")
            script = state.get("script")
            if not account or script not in registry.SCRIPTS:
                continue
            if state.get("status") in ("finished", "stopped", "failed", "needs_human"):
                continue
            with self.lock:
                if account in self.jobs:
                    continue
                job = Job(account, script, state.get("args") or [])
                self.jobs[account] = job
                self.spawn(job, script)
                self.write_state(job)
            self.start_supervisor(job)
            print("restored %s (%s)" % (account, script), flush=True)

    # -- socket ----------------------------------------------------------

    def serve(self):
        if os.path.exists(SOCKET_PATH):
            os.unlink(SOCKET_PATH)
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(SOCKET_PATH)
        server.listen(8)
        self.server = server
        with open(PID_PATH, "w") as handle:
            handle.write(str(os.getpid()))
        self.restore_jobs()
        holdings.start_scheduler()
        web.start_web(self.dispatch, int(os.environ.get("LIFESCAPE_WEB_PORT", "8765")))
        print("orchestrator listening on %s" % SOCKET_PATH, flush=True)
        try:
            while self.running:
                try:
                    connection, _address = server.accept()
                except OSError:
                    break  # socket closed by shutdown
                threading.Thread(target=self.serve_client, args=(connection,), daemon=True).start()
        finally:
            server.close()
            try:
                os.unlink(SOCKET_PATH)
            except OSError:
                pass
        return 0

    def serve_client(self, connection):
        with connection:
            reader = connection.makefile("r")
            writer = connection.makefile("w")
            try:
                request = json.loads(reader.readline() or "{}")
                reply = self.dispatch(request)
            except Exception as error:
                reply = {"ok": False, "error": repr(error)}
            writer.write(json.dumps(reply) + "\n")
            writer.flush()

    def dispatch(self, request):
        command = request.get("cmd")
        account = request.get("account") or ""
        if command == "start":
            return self.start_job(account, request.get("script") or "", request.get("args") or [])
        if command == "stop":
            return self.stop_job(account)
        if command == "status":
            with self.lock:
                live = {job.account: job for job in self.jobs.values()}
            if account:
                accounts = [account] if account in live or os.path.exists(user_paths(account)[0]) else []
            else:
                accounts = sorted(set(live) | set(known_accounts()))
            described = []
            for name in accounts:
                job = live.get(name)
                if job is None or job.status in ("finished", "stopped"):
                    state = read_json(user_paths(name)[0], {}) or {}
                    # Only an account with a fresh heartbeat is online: the
                    # list follows connect/disconnect instead of keeping
                    # every account ever supervised.
                    age = time.time() - (state.get("updated") or 0)
                    if age > ONLINE_WINDOW:
                        continue
                    job = Job(name, state.get("script") or "", state.get("args") or [])
                    job.status = "idle"
                entry = job.describe()
                entry["account_name"] = naming.humanize(entry["account"])
                described.append(entry)
            # Running bots first: the idle snapshots are only kept so an
            # account stays spectatable between runs.
            described.sort(key=lambda entry: (entry["status"] == "idle", entry["account"]))
            return {"ok": True, "jobs": described}
        if command == "items":
            return {"ok": True, "data": holdings.snapshot()}
        if command == "show":
            merged_path, _partial, _log = user_paths(account)
            return {"ok": True, "state": naming.resolve_state(read_json(merged_path, {}))}
        if command == "live":
            # Spectator feed: the heartbeat is written 5x/s by the SDK, the
            # merged file only every 0.5s, so overlay it to look real-time.
            merged_path, partial_path, _log = user_paths(account)
            state = read_json(merged_path, {}) or {}
            state.update(read_json(partial_path, {}) or {})
            return {"ok": True, "state": naming.resolve_state(state)}
        if command == "logs":
            _merged, _partial, log_path = user_paths(account)
            lines = int(request.get("lines") or 40)
            try:
                with open(log_path, "r", errors="replace") as handle:
                    tail = handle.readlines()[-lines:]
            except OSError:
                tail = []
            return {"ok": True, "lines": [line.rstrip("\n") for line in tail]}
        if command == "shutdown":
            with self.lock:
                jobs = list(self.jobs.values())
            for job in jobs:
                self.stop_job(job.account)
            self.running = False
            try:
                self.server.close()  # unblock accept()
            except OSError:
                pass
            return {"ok": True}
        return {"ok": False, "error": "unknown command %r" % (command,)}


if __name__ == "__main__":
    sys.exit(Orchestrator().serve())
