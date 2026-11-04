"""Supervisor daemon: one persistent session worker per account.

It owns session lifetime (open/close), the log tail that carries
``@@EVENT`` markers from ``scripts/randomevents.py``, the rescue state
machine (unresolved event -> rescue in session -> resume the interrupted
script), and the unix socket the CLI talks to. It never speaks the game
protocol: that stays in the SDK, inside the session worker.
"""

import json
import os
import signal
import socket
import subprocess
import sys
import threading
import traceback
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
    session_paths,
    user_paths,
    write_json,
)

STOP_GRACE = 8.0
OPEN_TIMEOUT = 45.0
WORKER_TIMEOUT = 40.0
# Process-side bounds in wall-clock seconds: spawning a worker and waiting
# for its socket RPCs happen outside any game connection, so no game-tick
# bound can apply here.


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


def tail_lines(path, lines, window=131072):
    """Last lines of a (possibly huge) log without reading it whole.

    The dashboard polls logs on every websocket tick: readlines() on a
    hundreds-of-MB file costs seconds of CPU per call and pins the
    daemon at 100%. Seeking to the tail bounds the work to ~window.
    """
    try:
        with open(path, "rb") as handle:
            handle.seek(0, 2)
            size = handle.tell()
            offset = max(0, size - max(window, lines * 2048))
            handle.seek(offset)
            data = handle.read()
    except OSError:
        return []
    found = data.decode("utf-8", "replace").splitlines()
    if offset and found:
        found = found[1:]  # first line started mid-line
    return found[-lines:] if lines else []


def _daemon_alive():
    """True when a live daemon answers on the orchestrator socket."""
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(2)
    try:
        client.connect(SOCKET_PATH)
        return True
    except OSError:
        return False
    finally:
        client.close()


class AttachedProcess:
    """A session worker that outlived the daemon: signal by pid."""

    def __init__(self, pid):
        self.pid = pid

    def poll(self):
        try:
            os.kill(self.pid, 0)
        except OSError:
            return 1
        return None

    def terminate(self):
        try:
            os.kill(self.pid, signal.SIGTERM)
        except OSError:
            pass

    def kill(self):
        try:
            os.kill(self.pid, signal.SIGKILL)
        except OSError:
            pass


class Orchestrator:
    def __init__(self):
        self.jobs = {}
        self.lock = threading.Lock()
        self.running = True
        ensure_dirs()

    # -- session control -----------------------------------------------

    def spawn(self, job, no_login=False):
        """Start the session worker and wait for its login (or typed error)."""
        sock_path, ready_path = session_paths(job.account)
        for stale in (sock_path, ready_path):
            try:
                os.unlink(stale)
            except OSError:
                pass
        _merged, _partial, log_path = user_paths(job.account)
        env = dict(
            os.environ,
            PYTHONPATH=ROOT,
            LIFESCAPE_USER=job.account,
            LIFESCAPE_STATE=user_paths(job.account)[1],
        )
        if no_login:
            env["LIFESCAPE_NO_LOGIN"] = "1"
        job.process = subprocess.Popen(
            [sys.executable, "-m", "orchestration.session_worker", job.account],
            cwd=ROOT,
            stdout=open(log_path, "ab"),
            stderr=subprocess.STDOUT,
            env=env,
            start_new_session=True,
        )
        job.status = "starting"
        job.started = time.time()
        print("session %s: worker pid=%d" % (job.account, job.process.pid), flush=True)
        deadline = time.time() + OPEN_TIMEOUT
        ready = None
        while time.time() < deadline:
            if job.process.poll() is not None:
                break
            try:
                with open(ready_path) as handle:
                    ready = json.load(handle)
                break
            except (OSError, ValueError):
                time.sleep(0.1)
        else:
            ready = None
        try:
            os.unlink(ready_path)
        except OSError:
            pass
        if not isinstance(ready, dict) or not ready.get("ok"):
            error = (ready or {}).get("error") if isinstance(ready, dict) else None
            error = error or "worker died during open"
            self.kill_process(job)
            with open(log_path, "ab") as handle:
                handle.write(("open failed for %s: %s\n" % (job.account, error)).encode())
            return {"ok": False, "error": error}
        return {"ok": True}

    def kill_process(self, job):
        process, job.process = job.process, None
        if process is None:
            return
        try:
            if process.poll() is None:
                process.terminate()
                deadline = time.time() + STOP_GRACE
                while time.time() < deadline and process.poll() is None:
                    time.sleep(0.2)
                if process.poll() is None:
                    process.kill()
        except OSError:
            pass

    def open_job(self, account, no_login=False):
        if not account:
            return {"ok": False, "error": "account is required"}
        with self.lock:
            existing = self.jobs.get(account)
            if existing is not None and existing.alive():
                return {"ok": False, "error": "already open %s" % account}
            elif existing is not None:
                del self.jobs[account]
        sock_path, _ready = session_paths(account)
        if os.path.exists(sock_path):
            try:
                self.send_worker(account, {"cmd": "ping"}, timeout=2)
                return {"ok": False, "error": "already open %s" % account}
            except OSError:
                pass
            try:
                os.unlink(sock_path)
            except OSError:
                pass
        job = Job(account)
        result = self.spawn(job, no_login=no_login)
        if not result.get("ok"):
            return result
        with self.lock:
            self.jobs[account] = job
        session = self.send_worker(account, {"cmd": "status"}, timeout=WORKER_TIMEOUT).get("session", {})
        self.apply_session(job, session)
        self.write_state(job)
        self.start_supervisor(job)
        return {"ok": True, "job": job.describe()}

    def start_supervisor(self, job):
        """One thread per account: accounts are watched in parallel."""
        job.supervisor = threading.Thread(target=self.supervise_job, args=(job,), daemon=True)
        job.supervisor.start()

    def run_job(self, account, script, args):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session, open %s first" % account}
        with job.guard():
            if job.rescue_for is not None:
                return {"ok": False, "error": "busy: rescue %s in progress" % job.rescue_for}
        try:
            registry.script_path(script)
        except KeyError as error:
            return {"ok": False, "error": str(error)}
        reply = self.send_worker(account, {"cmd": "run", "script": script, "args": args}, timeout=WORKER_TIMEOUT)
        if reply.get("ok"):
            self.apply_session(job, reply.get("session", {}))
            self.write_state(job)
        return reply

    def unload_job(self, account):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session for %s" % account}
        reply = self.send_worker(account, {"cmd": "unload"}, timeout=WORKER_TIMEOUT)
        if reply.get("ok"):
            self.apply_session(job, reply.get("session", {}))
            self.write_state(job)
        return reply

    def pause_job(self, account):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session for %s" % account}
        reply = self.send_worker(account, {"cmd": "pause"}, timeout=WORKER_TIMEOUT)
        if reply.get("ok"):
            self.apply_session(job, reply.get("session", {}))
            self.write_state(job)
        return reply

    def resume_job(self, account):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session for %s" % account}
        reply = self.send_worker(account, {"cmd": "resume"}, timeout=WORKER_TIMEOUT)
        if reply.get("ok"):
            self.apply_session(job, reply.get("session", {}))
            self.write_state(job)
        return reply

    def call_job(self, account, method, args):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session, open %s first" % account}
        return self.send_worker(
            account, {"cmd": "call", "method": method, "args": args, "admin": account == "admin"},
            timeout=WORKER_TIMEOUT,
        )

    def methods_job(self, account):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session, open %s first" % account}
        return self.send_worker(account, {"cmd": "methods", "admin": account == "admin"}, timeout=WORKER_TIMEOUT)

    def close_job(self, account):
        with self.lock:
            job = self.jobs.get(account)
        if job is None:
            return {"ok": False, "error": "no session for %s" % account}
        job.supervise = False
        try:
            reply = self.send_worker(account, {"cmd": "close"}, timeout=WORKER_TIMEOUT)
            session = reply.get("session", {}) if reply.get("ok") else {}
        except OSError as error:
            session = {}
            print("close %s: worker unreachable: %s" % (account, error), flush=True)
        self.kill_process(job)
        for stale in session_paths(account):
            try:
                os.unlink(stale)
            except OSError:
                pass
        job.status = "stopped"
        job.phase = "idle"
        job.script = None
        job.paused = False
        job.supervise = False
        if isinstance(session, dict):
            if session.get("last_error"):
                job.last_error = session["last_error"]
        self.write_state(job)
        with self.lock:
            self.jobs.pop(account, None)
        return {"ok": True, "job": job.describe()}

    def send_worker(self, account, request, timeout=WORKER_TIMEOUT):
        """One JSON round-trip to the session worker.

        A missing socket means no live session, never a bare traceback.
        """
        sock_path, _ready = session_paths(account)
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(timeout)
        reader = writer = None
        try:
            try:
                client.connect(sock_path)
            except FileNotFoundError:
                raise ConnectionError("session unreachable for %s" % account)
            writer = client.makefile("w")
            writer.write(json.dumps(request) + "\n")
            writer.flush()
            reader = client.makefile("r")
            return json.loads(reader.readline() or "{}")
        finally:
            for stream in (reader, writer):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass
            client.close()

    def apply_session(self, job, session):
        """Mirror the worker's session truth (phase/script/status/errors)."""
        if not session:
            return
        with job.guard():
            job.phase = session.get("phase", job.phase)
            job.script = session.get("script", job.script)
            job.script_args = session.get("script_args", job.script_args)
            job.args = list(job.script_args)
            job.paused = bool(session.get("paused", job.paused))
            job.status = session.get("status", job.status)
            if session.get("last_error") is not None:
                job.last_error = session["last_error"]
            if session.get("last_event") is not None:
                job.last_event = session["last_event"]
            if session.get("last_code") is not None:
                job.last_code = session["last_code"]

    # -- supervision -----------------------------------------------------

    def supervise_job(self, job):
        while self.running and job.supervise:
            try:
                self.supervise_once(job)
            except Exception as error:
                print("supervise error on %s: %r" % (job.account, error), flush=True)
                traceback.print_exc()
            time.sleep(0.5)

    def supervise_once(self, job):
        process = job.process
        if process is not None and process.poll() is not None:
            self.handle_worker_death(job)
            return
        try:
            session = self.send_worker(job.account, {"cmd": "status"}, timeout=5).get("session")
        except OSError:
            session = None
        if session:
            self.apply_session(job, session)
        self.read_events(job)
        with job.guard():
            self.maybe_rescue_from_worker(job, session)
            self.detect_state_events(job)
            self.rescue_step(job)
            self.auto_idle_finished(job)
        # The merged state file is only read by show/CLI: rescue detection
        # above already ran, so a 2 s-old file is fine and this halves the
        # fsync churn of the 0.5 s supervision loop.
        now = time.time()
        if now - getattr(job, "_state_written_at", 0) >= 2.0:
            job._state_written_at = now
            self.write_state(job)

    def detect_state_events(self, job):
        """Launch a rescue for a teleport event no script reported.

        Detection reads the live state file, so it covers every session
        state: idle, walkto/probes/goto, even a failed one. Quest scripts
        own their events through check() (inline solve, then report), and
        an active rescue must never be clobbered, so both are skipped.
        Cubes never teleport: only idle sessions get the cube rescue, a
        walkto keeps walking with boxes in its pack. The synthesized event
        reuses handle_event, hence the same unload/mapping/resume path.

        Towers are walkable ground (Edgeville, banks): a bot may stand on
        a fail spot legitimately. Only a teleport arrival counts (position
        jump), otherwise every pass re-fires on the same tiles forever.
        Maze/mime stages are instances: any presence there is a yank.
        """
        from scripts import randomevents
        if job.rescue_for is not None:
            return
        script = job.script or ""
        if script.startswith(("quest_", "rescue_")):
            return
        if job.status in ("failed", "disconnected", "reconnecting", "busy", "starting", "stopped"):
            return
        merged_path, partial_path, _log = user_paths(job.account)
        state = read_json(merged_path, {}) or {}
        state.update(read_json(partial_path, {}) or {})
        pos = state.get("position") or []
        event = None
        if len(pos) > 2:
            _level, x, z = pos[0], pos[1], pos[2]
            if randomevents.MAZE_X[0] <= x < randomevents.MAZE_X[1] and \
                    randomevents.MAZE_Z[0] <= z < randomevents.MAZE_Z[1]:
                event = "maze"
            elif randomevents.MIME_X[0] <= x < randomevents.MIME_X[1] and \
                    randomevents.MIME_Z[0] <= z < randomevents.MIME_Z[1]:
                event = "mime"
            elif (_level, x, z) in randomevents.FAIL_TELEPORTS:
                last = getattr(job, "_event_last_pos", None)
                if last is None or abs(x - last[0]) + abs(z - last[1]) > 100:
                    event = "tower"
            job._event_last_pos = (x, z)
        if event is None and not script:
            inv = state.get("inv") or []
            if any((entry.get("name") if isinstance(entry, dict) else entry) == "macro_cube" for entry in inv):
                event = "cube"
        if event is None:
            return
        now = time.time()
        if now - getattr(job, "_event_detect_at", 0) < 60:
            return
        job._event_detect_at = now
        print("job %s: state-detected %s at %s" % (job.account, event, pos), flush=True)
        self.handle_event(job, {"kind": "random_event", "event": event, "resolved": False})

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
        previous = job.last_event if isinstance(job.last_event, dict) else {}
        job.last_event = {"event": name, "resolved": resolved, "at": time.time()}
        for key in ("script", "args"):
            if key in previous:
                job.last_event[key] = previous[key]
        if resolved:
            return
        with job.guard():
            if job.rescue_for is not None:
                return
            if job.phase == "running" and job.script is not None:
                job.interrupted = (job.script, list(job.script_args))
                self.send_worker(job.account, {"cmd": "unload"}, timeout=WORKER_TIMEOUT)
            elif isinstance(job.last_event, dict) and job.last_event.get("script"):
                # The worker already aborted the script before the daemon
                # read this event (idle mirror, script cleared): resume from
                # the script it retained in its own event record.
                job.interrupted = (job.last_event["script"], list(job.last_event.get("args") or []))
            rescue = registry.RESCUES.get(name, "rescue_fallback")
            try:
                reply = self.send_worker(job.account, {"cmd": "run", "script": rescue, "args": []}, timeout=WORKER_TIMEOUT)
            except OSError as error:
                reply = {"ok": False, "error": str(error)}
            if reply.get("ok"):
                job.rescue_for = rescue
                job.phase = "running"
                job.script = rescue
                job.script_args = []
                job.args = []
                job.status = "running"
            else:
                job.last_error = {"where": "rescue", "type": "rescue", "message": str(reply.get("error")), "at": []}

    def maybe_rescue_from_worker(self, job, session):
        """Rescue when the worker aborted on a random event without @@EVENT.

        The normal path prints @@EVENT before raising, but a script that
        raises SystemExit random event without reporting leaves only the
        worker last_event. Start the fallback rescue from it instead of
        leaving the session idle with an unresolved event.
        """
        if not isinstance(session, dict):
            return
        if job.rescue_for is not None:
            return
        last = session.get("last_event")
        if not isinstance(last, dict):
            return
        if last.get("event") != "random_event" or last.get("resolved"):
            return
        if session.get("phase") != "idle" or session.get("script") is not None:
            return
        script = last.get("script")
        if not script:
            return
        if last.get("at") == job._worker_rescue_at:
            return
        job._worker_rescue_at = last.get("at")
        job.interrupted = (script, list(last.get("args") or []))
        try:
            reply = self.send_worker(job.account, {"cmd": "run", "script": "rescue_fallback", "args": []}, timeout=WORKER_TIMEOUT)
        except OSError as error:
            reply = {"ok": False, "error": str(error)}
        if reply.get("ok"):
            job.rescue_for = "rescue_fallback"
            job.phase = "running"
            job.script = "rescue_fallback"
            job.script_args = []
            job.args = []
            job.status = "running"
        else:
            job.last_error = {"where": "rescue", "type": "rescue", "message": str(reply.get("error")), "at": []}

    def auto_idle_finished(self, job):
        """Finished is transient: the daemon unloads it back to idle.

        The worker keeps finished until observed so the test command and
        the dashboard see it; the next supervision pass returns the
        session to idle without dropping the connection, keeping last_code.
        """
        if job.rescue_for is not None:
            return
        if job.phase != "idle" or job.status != "finished":
            return
        try:
            reply = self.send_worker(job.account, {"cmd": "unload"}, timeout=WORKER_TIMEOUT)
        except OSError:
            return
        if reply.get("ok"):
            self.apply_session(job, reply.get("session", {}))
        if job.status == "finished":
            job.status = "idle"

    def rescue_step(self, job):
        """A finished rescue hands the session back; a failed one ends it."""
        if job.rescue_for is None:
            return
        if job.phase == "running":
            return
        rescue = job.rescue_for
        job.rescue_for = None
        if job.status == "failed":
            job.interrupted = None
            print("job %s: rescue %s failed, session failed" % (job.account, rescue), flush=True)
            job.status = "failed"
            return
        if job.interrupted is not None:
            script, args = job.interrupted
            job.interrupted = None
            try:
                reply = self.send_worker(job.account, {"cmd": "run", "script": script, "args": args}, timeout=WORKER_TIMEOUT)
            except OSError as error:
                reply = {"ok": False, "error": str(error)}
            if reply.get("ok"):
                self.apply_session(job, reply.get("session", {}))
            else:
                job.last_error = {"where": "resume", "type": "resume", "message": str(reply.get("error")), "at": []}
                job.status = "failed"
        else:
            job.status = "idle"

    def handle_worker_death(self, job):
        code = job.process.poll() if job.process is not None else None
        job.process = None
        job.supervise = False
        job.phase = "idle"
        job.script = None
        job.status = "disconnected"
        job.last_error = {"where": "worker", "type": "death", "message": "session worker died (code %s): close + open to recover" % (code,), "at": []}
        print("job %s: worker died (code %s)" % (job.account, code), flush=True)
        self.write_state(job)

    def write_state(self, job):
        merged_path, _partial, _log = user_paths(job.account)
        write_json(merged_path, job.describe())

    def restore_jobs(self):
        """Adopt live session workers as idle, without their scripts.

        Workers survive the daemon: adopt each reachable one, stop any
        script it still runs (the new daemon never resumes work it did
        not start), and report the session idle. Stale sockets are removed.
        """
        for account in known_accounts():
            with self.lock:
                if account in self.jobs:
                    continue
            sock_path, _ready = session_paths(account)
            if not os.path.exists(sock_path):
                continue
            try:
                session = self.send_worker(account, {"cmd": "status"}, timeout=3).get("session")
            except OSError:
                try:
                    os.unlink(sock_path)
                except OSError:
                    pass
                continue
            if not session:
                continue
            if session.get("phase") == "running":
                try:
                    self.send_worker(account, {"cmd": "unload"}, timeout=WORKER_TIMEOUT)
                    session = self.send_worker(account, {"cmd": "status"}, timeout=3).get("session") or session
                except OSError:
                    pass
            with self.lock:
                if account in self.jobs:
                    continue
                job = Job(account)
                job.process = AttachedProcess(session.get("session_pid") or 0)
                self.jobs[account] = job
                self.apply_session(job, session)
                job.phase = "idle"
                job.script = None
                job.script_args = []
                job.args = []
                if job.status == "running":
                    job.status = "idle"
                try:
                    with open(user_paths(account)[2], "rb") as handle:
                        handle.seek(0, 2)
                        job.log_offset = handle.tell()
                except OSError:
                    pass
            self.start_supervisor(job)
            print("restored %s (%s)" % (account, job.script or "idle"), flush=True)

    # -- socket ----------------------------------------------------------

    def serve(self):
        if os.path.exists(SOCKET_PATH):
            if _daemon_alive():
                print("daemon already running, refusing to start", flush=True)
                return 1
            try:
                os.unlink(SOCKET_PATH)
            except OSError:
                pass  # stale socket, take over below
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(SOCKET_PATH)
        server.listen(8)
        self.server = server
        with open(PID_PATH, "w") as handle:
            handle.write(str(os.getpid()))
        # The web port must bind before workers are adopted: a bind
        # failure exits here with every session untouched, instead of
        # leaving a half-started daemon that already unloaded scripts.
        web.start_web(self.dispatch, int(os.environ.get("LIFESCAPE_WEB_PORT", "8765")))
        self.restore_jobs()
        holdings.start_scheduler()
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
            # makefile() dups the fd: close both explicitly (PyPy's GC is
            # lazy, an unclosed pair leaks 2 fds per command and select()
            # breaks past FD_SETSIZE).
            reader = connection.makefile("r")
            writer = connection.makefile("w")
            try:
                request = json.loads(reader.readline() or "{}")
                reply = self.dispatch(request)
            except Exception as error:
                reply = {"ok": False, "error": repr(error)}
            try:
                writer.write(json.dumps(reply) + "\n")
                writer.flush()
            except OSError:
                pass
            finally:
                for stream in (reader, writer):
                    try:
                        stream.close()
                    except OSError:
                        pass

    def test_command(self, pattern):
        reply = self.open_job("tests", no_login=True)
        if not reply.get("ok"):
            return reply
        reply = self.send_worker("tests", {"cmd": "run", "script": "tests", "args": pattern}, timeout=WORKER_TIMEOUT)
        if not reply.get("ok"):
            self.close_job("tests")
            return reply
        deadline = time.time() + 600
        state = {}
        while time.time() < deadline:
            time.sleep(0.5)
            state = self.dispatch({"cmd": "show", "account": "tests"}).get("state", {})
            if state.get("status") in ("finished", "failed"):
                break
            if state.get("status") == "idle" and state.get("last_code") is not None:
                break
        code = 0 if state.get("status") == "finished" or (state.get("status") == "idle" and state.get("last_code") == 0) else 1
        tail = self.dispatch({"cmd": "logs", "account": "tests", "lines": 60}).get("lines", [])
        self.close_job("tests")
        return {"ok": code == 0, "state": state, "log_tail": tail}

    def dispatch(self, request):
        command = request.get("cmd")
        account = request.get("account") or ""
        if command == "open":
            return self.open_job(account)
        if command == "run":
            return self.run_job(account, request.get("script") or "", request.get("args") or [])
        if command == "unload":
            return self.unload_job(account)
        if command == "pause":
            return self.pause_job(account)
        if command == "resume":
            return self.resume_job(account)
        if command == "close":
            return self.close_job(account)
        if command == "call":
            return self.call_job(account, request.get("method") or "", request.get("args") or [])
        if command == "methods":
            return self.methods_job(account)
        if command == "status":
            with self.lock:
                live = {job.account: job for job in self.jobs.values()}
            if account:
                accounts = [account] if account in live else []
            else:
                accounts = sorted(live)
            described = []
            for name in accounts:
                entry = live[name].describe()
                entry["account_name"] = naming.humanize(entry["account"])
                described.append(entry)
            described.sort(key=lambda entry: (entry["status"] == "idle", entry["account"]))
            return {"ok": True, "jobs": described}
        if command == "items":
            return {"ok": True, "data": holdings.snapshot()}
        if command == "scripts":
            registry.reload_if_changed()
            return {"ok": True, "scripts": sorted(registry.SCRIPTS)}
        if command == "accounts":
            live = sorted(self.jobs)
            known = sorted(set(known_accounts()) | set(live))
            return {"ok": True, "accounts": known, "live": live}
        if command == "show":
            merged_path, _partial, _log = user_paths(account)
            return {"ok": True, "state": naming.resolve_state(read_json(merged_path, {}))}
        if command == "live":
            merged_path, partial_path, _log = user_paths(account)
            state = read_json(merged_path, {}) or {}
            state.update(read_json(partial_path, {}) or {})
            return {"ok": True, "state": naming.resolve_state(state)}
        if command == "logs":
            _merged, _partial, log_path = user_paths(account)
            lines = int(request.get("lines") or 40)
            return {"ok": True, "lines": tail_lines(log_path, lines)}
        if command == "test":
            return self.test_command(request.get("args") or [])
        if command == "shutdown":
            with self.lock:
                jobs = list(self.jobs.values())
            for job in jobs:
                try:
                    self.close_job(job.account)
                except OSError:
                    pass
            self.running = False
            try:
                self.server.close()  # unblock accept()
            except OSError:
                pass
            return {"ok": True}
        return {"ok": False, "error": "unknown command %r" % (command,)}


if __name__ == "__main__":
    sys.exit(Orchestrator().serve())
