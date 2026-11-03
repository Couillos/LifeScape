"""Persistent game session: one connected account, many scripts over time.

Usage (daemon only):

    python -m orchestration.session_worker <account>

The daemon spawns one worker per ``open``. The worker logs in once, then
serves commands on its unix socket until ``close``: ``run`` starts a
supervised script's ``run(bot, *args)`` on a dedicated thread,
``unload`` stops it cooperatively without dropping the connection,
``call`` runs one whitelisted SDK method (queued to the next ``pump``
safe-point when a script runs, immediately from the heartbeat snapshot
for reads), ``status``/``methods`` describe the session.

The script thread is the only pumper while it lives; the main thread
pumps only when idle. Reads never touch the live Bot off its thread:
they are answered from the last heartbeat snapshot.
"""

import importlib.util
import json
import os
import select
import socket
import sys
import threading
import time
import traceback

from lifescape.client import Bot
from lifescape.client.core import SAFE_POINT_TICKS, UNLOAD_TICKS, StaleConnection, StopRequested
from lifescape.client import snapshot as snapshot_mod
from lifescape.connection import LoginError

from . import registry
from .paths import ROOT, ensure_dirs, session_paths, user_paths

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

WALL_BACKSTOP = 30.0  # process guard when ticks are frozen (offline/CPU spin)

READ_METHODS = ("inventory", "bank_items", "backpack", "chat_texts", "chat_options", "combat_level", "position")

# Session internals, not script calls: snapshot/queue_command/drop_command
# drive the pump safe-point from the inside, while request/stop/clear/
# invalidate steer the worker's own thread protocol. Exposing them through
# call would let one script read torn state or break another run's unload.
DENY_METHODS = {
    "login", "relogin", "close",
    "snapshot", "queue_command", "drop_command",
    "request_stop", "stop_requested", "clear_stop", "invalidate_sends",
}


def ensure_project_paths():
    """Put the project on sys.path for supervised scripts.

    Every script runs through this worker, so scripts import ``lifescape``,
    ``scripts`` and ``gamedata`` directly with no per-file boilerplate. The
    inner ``gamedata/`` dir comes first: ``import gamedata`` must resolve to
    the generated tables, shadowing the outer container.
    """
    for entry in (PROJECT_ROOT, os.path.join(PROJECT_ROOT, "gamedata")):
        if entry not in sys.path:
            sys.path.insert(0, entry)


def parse_call_args(raw):
    """CLI strings to values: JSON when it parses, else the raw string."""
    parsed = []
    for item in raw:
        try:
            parsed.append(json.loads(item))
        except (ValueError, TypeError):
            parsed.append(item)
    return parsed


def read_from_snapshot(snapshot, method):
    """Answer a read-only call from heartbeat data, never touching the Bot."""
    if not snapshot:
        raise KeyError("no snapshot yet")
    if method == "position":
        return list(snapshot["position"])
    if method == "inventory":
        return snapshot_mod.aggregate(snapshot["inv"])
    if method == "bank_items":
        return snapshot_mod.bank_items(snapshot["inv"], snapshot["com_ids"])
    if method == "backpack":
        return snapshot_mod.backpack_view(snapshot["inv"], snapshot["inv_cap"])
    if method == "chat_texts":
        return sorted(snapshot["texts"].items())
    if method == "chat_options":
        return snapshot_mod.chat_options(dict(snapshot["texts"]))
    if method == "combat_level":
        return snapshot_mod.combat_level(snapshot["stats"])
    raise KeyError("unknown method %r" % (method,))


def aggregate(inv):
    return snapshot_mod.aggregate(inv)


def find_backpack(snapshot):
    return snapshot_mod.backpack_view(snapshot["inv"], snapshot["inv_cap"])


def chat_options(texts):
    return snapshot_mod.chat_options(dict(texts))


def method_lists(admin=False):
    """(reads, writes) whitelisted for call/shell."""
    reads = sorted(READ_METHODS)
    writes = sorted(
        name for name in dir(Bot)
        if not name.startswith("_") and callable(getattr(Bot, name)) and name not in DENY_METHODS
        and (admin or name != "cheat")
    )
    if not admin:
        writes = [name for name in writes if name != "cheat"]
    return reads, writes


class SessionWorker:
    def __init__(self, account):
        self.account = account
        self.bot = None
        self.connected = False
        self.phase = "idle"  # idle | running
        self.script = None
        self.script_args = []
        self.status = "starting"  # idle|running|busy|reconnecting|finished|failed|disconnected|starting|stopped
        self.last_error = None
        self.last_event = None
        self.last_code = None
        self.script_thread = None
        self.closing = False
        self._lock = threading.RLock()
        self._net_lock = threading.RLock()
        self._relogin_abort = threading.Event()
        self._unload_tick = None
        self.server = None
        ensure_dirs()
        _merged, _partial, self.log_path = user_paths(account)
        self.sock_path, self.ready_path = session_paths(account)

    def log(self, message):
        with open(self.log_path, "ab") as handle:
            handle.write(("%s\n" % message).encode())

    # -- lifetime ------------------------------------------------------

    def start(self):
        self.log("=== session open %s ===" % self.account)
        self.bind()
        if os.environ.get("LIFESCAPE_NO_LOGIN") != "1":
            self.bot = Bot()
            try:
                self.bot.login(self.account, self.account)
                self.bot.intend("connected")
            except Exception as error:
                self.write_ready(False, describe_open_error(error))
                try:
                    self.bot.close()
                except Exception:
                    pass
                return 1
            self.connected = True
        else:
            self.bot = Bot()
        with self._lock:
            self.status = "idle"
        self.write_ready(True, "")
        self.serve()
        return 0

    def write_ready(self, ok, error):
        with open(self.ready_path, "w") as handle:
            json.dump({"ok": ok, "error": error}, handle)

    # -- command socket --------------------------------------------------

    def bind(self):
        """Bind the command socket before login: the daemon's first command
        must never arrive before the socket exists (ready means reachable)."""
        try:
            os.unlink(self.sock_path)
        except OSError:
            pass
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.sock_path)
        self.server.listen(8)

    def serve(self):
        try:
            while not self.closing:
                readable, _w, _e = select.select([self.server], [], [], 0.2)
                if readable:
                    connection, _address = self.server.accept()
                    thread = threading.Thread(target=self.serve_client, args=(connection,), daemon=True)
                    thread.start()
                try:
                    self.idle_pump()
                except Exception as error:
                    print("idle pump failed, session kept: %r" % (error,), flush=True)
        finally:
            try:
                self.server.close()
            except OSError:
                pass
            try:
                os.unlink(self.sock_path)
            except OSError:
                pass

    def serve_client(self, connection):
        with connection:
            # makefile() dups the fd: close both explicitly (PyPy's GC is
            # lazy, an unclosed pair leaks 2 fds per command and select()
            # breaks past FD_SETSIZE).
            reader = connection.makefile("r")
            writer = connection.makefile("w")
            try:
                try:
                    request = json.loads(reader.readline() or "{}")
                    reply = self.dispatch(request)
                except Exception as error:
                    reply = {"ok": False, "error": repr(error)}
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

    def dispatch(self, request):
        command = request.get("cmd")
        if command == "ping":
            return {"ok": True}
        if command == "status":
            return {"ok": True, "session": self.describe()}
        if command == "methods":
            reads, writes = method_lists(admin=request.get("admin", False))
            return {"ok": True, "reads": reads, "writes": writes}
        if command == "run":
            return self.cmd_run(request.get("script") or "", request.get("args") or [])
        if command == "unload":
            return self.cmd_unload()
        if command == "call":
            return self.cmd_call(request.get("method") or "", request.get("args") or [], request.get("admin", False))
        if command == "close":
            return self.cmd_close()
        return {"ok": False, "error": "unknown command %r" % (command,)}

    def describe(self):
        with self._lock:
            return {
                "account": self.account,
                "phase": self.phase,
                "script": self.script,
                "script_args": list(self.script_args),
                "status": self.status,
                "session_pid": os.getpid(),
                "last_error": self.last_error,
                "last_event": self.last_event,
                "last_code": self.last_code,
                "ticks": self.bot.world.ticks if self.bot is not None else 0,
            }

    # -- run / unload ----------------------------------------------------

    def cmd_run(self, script, args):
        with self._lock:
            if self.status in ("busy", "reconnecting", "disconnected"):
                return {"ok": False, "error": "%s is %s" % (self.account, self.status)}
            if self.script_thread is not None and self.script_thread.is_alive():
                return {"ok": False, "error": "busy: %s is running" % (self.script,)}
            try:
                run_fn, _label = registry.load_run(script)
            except KeyError as error:
                return {"ok": False, "error": str(error)}
            self.bot.clear_stop()
            self._relogin_abort.clear()
            self.script = script
            self.script_args = list(args)
            self.phase = "running"
            self.status = "running"
            self.last_code = None
            os.environ["LIFESCAPE_SCRIPT"] = script
            thread = threading.Thread(target=self.run_thread, args=(run_fn, script, list(args)), daemon=True)
            self.script_thread = thread
        self.log("=== session run %s %s ===" % (self.account, script))
        thread.start()
        return {"ok": True, "session": self.describe()}

    def run_thread(self, run_fn, script, args):
        self.bot.note_run_thread()
        outcome = None
        is_rescue = script.startswith("rescue_")
        try:
            result = run_fn(self.bot, *args)
            code = result if isinstance(result, int) else 0
            outcome = ("finished" if code == 0 else "failed", code, None if code == 0 else {"where": script, "type": "exit", "message": "run returned %d" % code, "at": list(self.bot.world.position)})
        except StopRequested:
            outcome = ("idle", None, None)
        except SystemExit as stop:
            if stop.code in (None, 0, True):
                outcome = ("finished", 0, None)
            elif isinstance(stop.code, str) and "random event" in stop.code:
                outcome = ("event", None, None)
            elif is_rescue:
                outcome = ("failed", stop.code, {"where": script, "type": "SystemExit", "message": str(stop.code), "at": list(self.bot.world.position)})
            else:
                outcome = ("idle", None, {"where": script, "type": "SystemExit", "message": str(stop.code), "at": list(self.bot.world.position)})
        except StaleConnection as error:
            outcome = ("idle", None, None)
            print("orphan send muted in %s: %s" % (script, error), flush=True)
        except (LoginError, ConnectionError, OSError) as error:
            print("connection lost during %s: %s" % (script, error), flush=True)
            with self._lock:
                if self.closing or self.bot.stop_requested():
                    self.script_thread = None
                    self.phase = "idle"
                    self.script = None
                    return
                self.status = "reconnecting"
            if self.reconnect_flow():
                with self._lock:
                    self.script_thread = None
                    self.phase = "idle"
                    self.script = None
                    self.status = "idle"
            else:
                with self._lock:
                    self.script_thread = None
                    self.phase = "idle"
                    self.script = None
            return
        except Exception as error:
            traceback.print_exc()
            kind = "failed" if is_rescue else "idle"
            outcome = (kind, None, {"where": script, "type": type(error).__name__, "message": str(error), "at": list(self.bot.world.position)})
            print("@@EVENT " + json.dumps({
                "kind": "script_error", "script": script,
                "type": type(error).__name__, "message": str(error),
                "at": list(self.bot.world.position), "target": getattr(self.bot, "_route_target", None),
            }), flush=True)
        with self._lock:
            kind, code, error = outcome
            self.script_thread = None
            self.phase = "idle"
            self.script = None
            self.script_args = []
            os.environ["LIFESCAPE_SCRIPT"] = ""
            if kind == "finished":
                self.status = "finished"
                self.last_code = code
            elif kind == "failed":
                self.status = "failed"
                self.last_code = code
                self.last_error = error
                self.log("=== session failed %s %s ===" % (self.account, error))
            elif kind == "event":
                self.status = "idle"
                self.last_event = {"event": "random_event", "resolved": False, "at": time.time(),
                                   "script": script, "args": list(args)}
            else:
                self.status = "idle"
                if error is not None:
                    self.last_error = error

    def cmd_unload(self):
        with self._lock:
            thread = self.script_thread
            if thread is None or not thread.is_alive():
                self.phase = "idle"
                self.script = None
                self.script_args = []
                if self.status in ("running", "finished"):
                    self.status = "idle"
                return {"ok": True, "session": self.describe()}
            self.status = "busy"
        self.log("=== session unload %s ===" % self.account)
        self._relogin_abort.set()
        self.bot.request_stop()
        start_tick = self.bot.world.ticks
        start_wall = time.time()
        while thread.is_alive():
            if self.bot.world.ticks - start_tick >= UNLOAD_TICKS or time.time() - start_wall > WALL_BACKSTOP:
                break
            thread.join(0.2)
        orphan = thread.is_alive()
        if orphan:
            self.bot.invalidate_sends(thread)
            thread.join(2)
        with self._lock:
            for command in list(self.bot._commands):
                self.bot.drop_command(command)
        self.bot.clear_stop()
        self._relogin_abort.clear()
        # Verify on this thread while script_thread is still set: the idle
        # pump loop keeps skipping, so only one thread ever reads the socket
        # (two concurrent pumps desync the cipher and poison the session).
        with self._net_lock:
            if self.connected:
                try:
                    self.bot.pump(1)
                except StopRequested:
                    pass
                except Exception:
                    self.reconnect_flow()
        with self._lock:
            self.script_thread = None
            self.phase = "idle"
            self.script = None
            self.script_args = []
            os.environ["LIFESCAPE_SCRIPT"] = ""
            if self.status in ("running", "busy"):
                self.status = "idle"
        if orphan:
            print("script thread survived unload, its sends are muted", flush=True)
        return {"ok": True, "session": self.describe()}

    # -- call ------------------------------------------------------------

    def cmd_call(self, method, args, admin):
        self.log("=== session call %s %s ===" % (self.account, method))
        reads, writes = method_lists(admin=admin)
        if method in reads:
            try:
                return {"ok": True, "result": read_from_snapshot(self.bot.snapshot(), method)}
            except KeyError as error:
                return {"ok": False, "error": str(error)}
        if method not in writes:
            if method == "cheat" and not admin:
                return {"ok": False, "error": "forbidden: cheat is staff only"}
            return {"ok": False, "error": "unknown method %r" % (method,)}
        with self._lock:
            if self.status in ("busy", "reconnecting", "disconnected"):
                return {"ok": False, "error": "busy: session is %s" % self.status}
            if self._unload_in_progress():
                return {"ok": False, "error": "busy: unload in progress"}
            running = self.script_thread is not None and self.script_thread.is_alive()
        command = self.bot.queue_command(method, parse_call_args(args))
        reached = self.wait_command(command)
        if reached != "done" or command["cancelled"]:
            if command["started"].is_set():
                # Started but the waiter gave up (frozen ticks mid-call):
                # leave it running, its own timeouts bound it.
                return {"ok": False, "error": "call %s stalled: ticks frozen" % method}
            self.bot.drop_command(command)
            return {"ok": False, "error": "busy: script did not yield within %d ticks" % SAFE_POINT_TICKS}
        if command["error"] is not None:
            return {"ok": False, "error": command["error"]}
        return {"ok": True, "result": command["result"]}

    def _unload_in_progress(self):
        return self.bot.stop_requested()

    def wait_command(self, command):
        """Wait for the next pump safe-point, then for the call to finish.

        Only reaching the safe-point is tick-bounded (500 ticks): once the
        call starts, its own tick timeouts bound it, so the wait follows
        tick progress instead of a fixed budget. The wall backstop only
        fires when ticks are frozen (dead connection or CPU-spinning
        script), in either phase.
        Returns "started", "done", or None (give up)."""
        start_tick = self.bot.world.ticks
        start_wall = time.time()
        while not command["started"].is_set():
            if command["done"].is_set():
                # Drained and finished before the waiter looked: nothing left
                # to wait for, report the call as done instead of stalling.
                return "done"
            if command["cancelled"]:
                return None
            if self.bot.world.ticks - start_tick >= SAFE_POINT_TICKS:
                return None
            if time.time() - start_wall > WALL_BACKSTOP:
                return None
            if self.bot.stop_requested():
                return None
            time.sleep(0.05)
        if not command["started"].is_set():
            return None
        last_tick = self.bot.world.ticks
        last_progress = time.time()
        while not command["done"].is_set():
            if command["cancelled"]:
                return None
            if self.bot.world.ticks != last_tick:
                last_tick = self.bot.world.ticks
                last_progress = time.time()
            if time.time() - last_progress > WALL_BACKSTOP:
                return None
            if self.bot.stop_requested():
                return None
            time.sleep(0.05)
        return "done"

    # -- network ---------------------------------------------------------

    def idle_pump(self):
        with self._lock:
            if self.script_thread is not None:
                return
            if not self.connected or self.status in ("busy", "reconnecting", "disconnected", "starting"):
                return
            if self.bot.stop_requested():
                return
        try:
            self.bot.pump(1)
        except StopRequested:
            pass
        except (LoginError, ConnectionError, OSError):
            self.reconnect_flow()

    def reconnect_flow(self):
        """Bounded relogin on the calling thread: no other thread pumps
        while this runs (script threads die on loss; idle pump skips the
        reconnecting status)."""
        with self._lock:
            if self.closing:
                return False
            self.status = "reconnecting"
        with self._net_lock:
            try:
                self.bot.relogin()
            except (LoginError, ConnectionError, OSError, StaleConnection) as error:
                with self._lock:
                    self.status = "disconnected"
                    self.last_error = {"where": "relogin", "type": type(error).__name__, "message": str(error), "at": list(self.bot.world.position)}
                self.log("=== session disconnected %s %s ===" % (self.account, error))
                return False
            except Exception as error:
                with self._lock:
                    self.status = "disconnected"
                    self.last_error = {"where": "relogin", "type": type(error).__name__, "message": str(error), "at": list(self.bot.world.position)}
                return False
        with self._lock:
            self.connected = True
            if self.status == "reconnecting":
                self.status = "idle"
        self.bot.intend("reconnected")
        return True

    # -- close -----------------------------------------------------------

    def cmd_close(self):
        self.cmd_unload()
        self.log("=== session close %s ===" % self.account)
        with self._lock:
            self.closing = True
            self.status = "stopped"
        try:
            self.bot.close()
        except (OSError, ConnectionError, StaleConnection) as error:
            print("logout failed on close, socket shut anyway: %s" % (error,), flush=True)
            with self._lock:
                self.last_error = {"where": "close", "type": type(error).__name__, "message": str(error), "at": []}
        except Exception as error:
            with self._lock:
                self.last_error = {"where": "close", "type": type(error).__name__, "message": str(error), "at": []}
        session = self.describe()
        thread = threading.Thread(target=self.shutdown_soon, daemon=True)
        thread.start()
        return {"ok": True, "session": session}

    def shutdown_soon(self):
        # Let the close reply flush, then end the serve loop (it unlinks the
        # socket itself). Never close the server socket from here: select
        # would raise and look like a crash.
        time.sleep(0.5)
        self.closing = True


def describe_open_error(error):
    if isinstance(error, LoginError):
        return "login failed: %s" % (error,)
    return "unreachable: %s" % (error,)


def main():
    if len(sys.argv) < 2:
        print("usage: -m orchestration.session_worker <account>", file=sys.stderr)
        return 2
    ensure_project_paths()
    worker = SessionWorker(sys.argv[1])
    return worker.start()


if __name__ == "__main__":
    sys.exit(main())
