"""CLI client for the orchestrator daemon (see orchestration/daemon.py).

Sessions are persistent: open one per account, then load/unload scripts
and call SDK methods inside it without reconnecting.

    orchestrator.py open <account>
    orchestrator.py run <account> <script> [args...]
    orchestrator.py unload <account>
    orchestrator.py close <account>
    orchestrator.py call <account> <method> [args...]
    orchestrator.py shell <account>
    orchestrator.py status [account]
    orchestrator.py show <account>
    orchestrator.py see <account>
    orchestrator.py logs <account> [lines]
    orchestrator.py test [pattern]
    orchestrator.py shutdown
    orchestrator.py ping

Call args parse as JSON when possible (3200, true, "text", [1,2]),
else stay strings. Every command auto-starts the daemon when needed.
"""

import json
import os
import shlex
import socket
import subprocess
import sys
import time

from .daemon import Orchestrator
from .paths import ROOT, RUN_DIR, SOCKET_PATH, ensure_dirs


def send_command(request, timeout=45):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(timeout)
    client.connect(SOCKET_PATH)
    writer = client.makefile("w")
    writer.write(json.dumps(request) + "\n")
    writer.flush()
    reply = client.makefile("r").readline()
    client.close()
    return json.loads(reply or "{}")


def daemon_alive():
    return os.path.exists(SOCKET_PATH)


def ensure_daemon():
    if daemon_alive():
        try:
            send_command({"cmd": "status"}, timeout=2)
            return True
        except OSError:
            pass
    ensure_dirs()
    subprocess.Popen(
        [sys.executable, "-m", "orchestration.daemon"],
        cwd=ROOT,
        stdout=open(os.path.join(RUN_DIR, "orchestrator.log"), "ab"),
        stderr=subprocess.STDOUT,
        env=dict(os.environ, PYTHONPATH=ROOT),
        start_new_session=True,
    )
    deadline = time.time() + 5
    while time.time() < deadline:
        if daemon_alive():
            try:
                send_command({"cmd": "status"}, timeout=2)
                return True
            except OSError:
                pass
        time.sleep(0.2)
    return False


def format_status(jobs):
    if not jobs:
        return "no session"
    rows = []
    for job in jobs:
        position = job.get("position")
        place = "%d,%d" % (position[1], position[2]) if position else "-"
        if job.get("zone"):
            place += " %s" % job["zone"]
        rows.append("%-14s %-14s %-13s hp=%-4s %-10s %s" % (
            job.get("account", "?"),
            job.get("script") or "-",
            job.get("status", "?"),
            job.get("hitpoints") if job.get("hitpoints") is not None else "-",
            place,
            job.get("intention") or "",
        ))
    return "\n".join(rows)


def format_seen(state):
    """One line per thing the bot currently sees, with positions."""
    if not state:
        return "no state"
    pos = state.get("position") or [0, 0, 0]
    here = (pos[1], pos[2]) if len(pos) > 2 else (0, 0)

    def dist(entry):
        return abs(entry.get("x", here[0]) - here[0]) + abs(entry.get("z", here[1]) - here[1])

    lines = [
        "%s hp=%s pos=(%d,%d,%d) %s :: %s" % (
            state.get("account", "?"),
            state.get("hitpoints", "-"),
            pos[0] if len(pos) > 0 else 0, here[0], here[1],
            state.get("status", "?"),
            state.get("intention") or "-",
        ),
        "zone %s (%s)" % (state.get("zone") or "-", state.get("zone_dist", "-")),
    ]
    for entry in sorted(state.get("npcs", []) or [], key=dist):
        lines.append("npc  %-24s (%d,%d) d=%d" % (
            entry.get("name") or entry.get("type", "?"),
            entry.get("x", "?"), entry.get("z", "?"), dist(entry)))
    for entry in sorted(state.get("locs", []) or [], key=dist):
        flags = "".join(part for part in ("act" if entry.get("act") else "", "block" if entry.get("block") else "") if part)
        lines.append("loc  %-24s (%d,%d) d=%d %s" % (
            entry.get("name") or entry.get("id", "?"),
            entry.get("x", "?"), entry.get("z", "?"), dist(entry), flags))
    for entry in sorted(state.get("objs", []) or [], key=dist):
        lines.append("obj  %-24s x%-3s (%d,%d) d=%d" % (
            entry.get("name") or entry.get("id", "?"), entry.get("count", 1),
            entry.get("x", "?"), entry.get("z", "?"), dist(entry)))
    for entry in sorted(state.get("players", []) or [], key=dist):
        lines.append("player %-22s (%d,%d) d=%d" % (
            entry.get("name") or "?", entry.get("x", "?"), entry.get("z", "?"), dist(entry)))
    for entry in state.get("inv", []) or []:
        lines.append("inv  %-24s x%s" % (entry.get("name") or entry.get("id", "?"), entry.get("count", 1)))
    for entry in state.get("worn", []) or []:
        lines.append("worn %-24s x%s" % (entry.get("name") or entry.get("id", "?"), entry.get("count", 1)))
    if state.get("modal") or state.get("modal_name"):
        lines.append("modal %s %s" % (state.get("modal_name") or "-", state.get("modal")))
    for line in (state.get("chat") or [])[-8:]:
        lines.append("chat %s" % str(line)[:160])
    for line in (state.get("messages") or [])[-6:]:
        lines.append("msg  %s" % str(line)[:160])
    if state.get("route_target"):
        lines.append("route_target %s" % (state.get("route_target"),))
    if state.get("last_event"):
        lines.append("last_event %s" % (state.get("last_event"),))
    if state.get("last_error"):
        lines.append("last_error %s" % (state.get("last_error"),))
    return "\n".join(lines)


def shell(account):
    """REPL over one open session: SDK calls with fresh state after each."""
    try:
        reply = send_command({"cmd": "methods", "account": account}, timeout=10)
    except OSError as error:
        print("session unreachable: %s" % (error,), file=sys.stderr)
        return 1
    if not reply.get("ok"):
        print(reply.get("error", "no session, open %s first" % account), file=sys.stderr)
        return 1
    reads = reply.get("reads", [])
    writes = reply.get("writes", [])
    print("session %s: %d reads (snapshot) + %d writes (safe-point). help lists them." % (account, len(reads), len(writes)))
    while True:
        try:
            line = input("%s> " % account)
        except EOFError:
            break
        except KeyboardInterrupt:
            # The session lives in its worker: ^C only gets a fresh prompt.
            print()
            continue
        try:
            parts = shlex.split(line)
        except ValueError as error:
            print("parse error: %s" % (error,))
            continue
        if not parts:
            continue
        if parts[0] == "exit":
            break
        if parts[0] == "help":
            print("reads:  " + " ".join(reads))
            print("writes: " + " ".join(writes))
            print("also:   unload, exit")
            continue
        if parts[0] == "unload":
            reply = send_command({"cmd": "unload", "account": account})
            print(json.dumps(reply, sort_keys=True))
            continue
        method, args = parts[0], parts[1:]
        try:
            reply = send_command({"cmd": "call", "account": account, "method": method, "args": args})
        except OSError as error:
            print("session unreachable: %s" % (error,))
            continue
        print(json.dumps(reply, sort_keys=True))
        try:
            state = send_command({"cmd": "show", "account": account}, timeout=10).get("state", {})
        except OSError:
            continue
        print("%s hp=%s pos=%s :: %s" % (
            state.get("status", "?"), state.get("hitpoints", "-"),
            state.get("position", "-"), state.get("intention") or "-"))
    return 0


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    command = argv[1]
    if command == "daemon":
        return Orchestrator().serve()
    if not ensure_daemon():
        print("orchestrator failed to start", file=sys.stderr)
        return 1
    account = argv[2] if len(argv) > 2 else ""
    if command == "open":
        reply = send_command({"cmd": "open", "account": account})
    elif command == "close":
        reply = send_command({"cmd": "close", "account": account})
    elif command == "run":
        script = argv[3] if len(argv) > 3 else ""
        reply = send_command({"cmd": "run", "account": account, "script": script, "args": argv[4:]})
    elif command == "unload":
        reply = send_command({"cmd": "unload", "account": account})
    elif command == "call":
        method = argv[3] if len(argv) > 3 else ""
        reply = send_command({"cmd": "call", "account": account, "method": method, "args": argv[4:]})
        print(json.dumps(reply, indent=2, sort_keys=True))
        return 0 if reply.get("ok") else 1
    elif command == "shell":
        return shell(account)
    elif command == "status":
        reply = send_command({"cmd": "status", "account": account})
        print(format_status(reply.get("jobs", [])))
        return 0 if reply.get("ok") else 1
    elif command == "show":
        reply = send_command({"cmd": "show", "account": account})
        print(json.dumps(reply.get("state", {}), indent=2, sort_keys=True))
        return 0
    elif command == "see":
        reply = send_command({"cmd": "show", "account": account})
        print(format_seen(reply.get("state", {}) or {}))
        return 0
    elif command == "logs":
        lines = int(argv[3]) if len(argv) > 3 else 40
        reply = send_command({"cmd": "logs", "account": account, "lines": lines})
        print("\n".join(reply.get("lines", [])))
        return 0
    elif command == "test":
        reply = send_command({"cmd": "test", "args": argv[2:]}, timeout=660)
        for line in reply.get("log_tail", []) or []:
            print(line)
        state = reply.get("state", {}) or {}
        print("tests: %s" % (state.get("status", "?")))
        return 0 if reply.get("ok") else 1
    elif command == "shutdown":
        reply = send_command({"cmd": "shutdown"})
    elif command == "ping":
        reply = send_command({"cmd": "status"})
        print("daemon ok" if reply.get("ok") else reply)
        return 0
    else:
        print("unknown command %r" % (command,), file=sys.stderr)
        return 2
    print(json.dumps(reply, indent=2, sort_keys=True))
    return 0 if reply.get("ok") else 1
