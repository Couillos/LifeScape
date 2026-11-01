"""CLI client for the orchestrator daemon (see orchestration/daemon.py).

    orchestrator.py start <script> <account> [args...]
    orchestrator.py stop <account>
    orchestrator.py restart <script> <account> [args...]
    orchestrator.py status [account]
    orchestrator.py show <account>
    orchestrator.py logs <account> [lines]
    orchestrator.py test [pattern]
    orchestrator.py shutdown
    orchestrator.py ping

Every command auto-starts the daemon when it is not running.
"""

import json
import os
import socket
import subprocess
import sys
import time

from .paths import ROOT, RUN_DIR, SOCKET_PATH, ensure_dirs


def send_command(request, timeout=10):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(timeout)
    client.connect(SOCKET_PATH)
    client.sendall((json.dumps(request) + "\n").encode())
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
        return "no job"
    rows = []
    for job in jobs:
        position = job.get("position")
        place = "%d,%d" % (position[1], position[2]) if position else "-"
        rows.append("%-14s %-12s %-11s hp=%-4s %-10s %s" % (
            job.get("account", "?"),
            job.get("script", "?"),
            job.get("status", "?"),
            job.get("hitpoints") if job.get("hitpoints") is not None else "-",
            place,
            job.get("intention") or "",
        ))
    return "\n".join(rows)


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    command = argv[1]
    if command == "daemon":
        from .daemon import Orchestrator
        return Orchestrator().serve()
    if not ensure_daemon():
        print("orchestrator failed to start", file=sys.stderr)
        return 1
    account = argv[2] if len(argv) > 2 else ""
    if command == "start":
        script = argv[2] if len(argv) > 2 else ""
        account = argv[3] if len(argv) > 3 else ""
        reply = send_command({"cmd": "start", "account": account, "script": script, "args": argv[4:]})
    elif command == "stop":
        reply = send_command({"cmd": "stop", "account": account})
    elif command == "restart":
        script = argv[2] if len(argv) > 2 else ""
        account = argv[3] if len(argv) > 3 else ""
        send_command({"cmd": "stop", "account": account})
        if not script or not account:
            state = send_command({"cmd": "show", "account": account}).get("state", {}) or {}
            script = script or state.get("script", "")
            account = account or state.get("account", "")
        reply = send_command({"cmd": "start", "account": account, "script": script, "args": argv[4:]})
    elif command == "status":
        reply = send_command({"cmd": "status", "account": account})
        print(format_status(reply.get("jobs", [])))
        return 0 if reply.get("ok") else 1
    elif command == "show":
        reply = send_command({"cmd": "show", "account": account})
        print(json.dumps(reply.get("state", {}), indent=2, sort_keys=True))
        return 0
    elif command == "logs":
        lines = int(argv[3]) if len(argv) > 3 else 40
        reply = send_command({"cmd": "logs", "account": account, "lines": lines})
        print("\n".join(reply.get("lines", [])))
        return 0
    elif command == "test":
        reply = send_command({"cmd": "start", "account": "tests", "script": "tests", "args": argv[2:]})
        if not reply.get("ok"):
            print(json.dumps(reply, indent=2, sort_keys=True))
            return 1
        while True:
            state = send_command({"cmd": "show", "account": "tests"}).get("state", {})
            if state.get("status") not in ("running", "starting", "restarting", "cleaning", "repairing"):
                break
            time.sleep(0.5)
        lines = send_command({"cmd": "logs", "account": "tests", "lines": 60}).get("lines", [])
        print("\n".join(lines))
        return 0 if state.get("status") == "finished" else 1
    elif command == "shutdown":
        reply = send_command({"cmd": "shutdown"})
    elif command == "ping":
        reply = send_command({"cmd": "status"})
        print("daemon ok" if reply.get("ok") else reply)
        return 0
    else:
        print(__doc__)
        return 2
    print(json.dumps(reply, indent=2, sort_keys=True))
    return 0 if reply.get("ok") else 1
