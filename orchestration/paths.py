"""Filesystem layout and JSON helpers for the bot orchestrator.

Everything the daemon writes lives under ``run/`` at the project root:

    run/users/<account>.json       merged live state (daemon view)
    run/users/<account>.bot.json   heartbeat written by the SDK
    run/logs/<account>.log         job output, appended across restarts
    run/orchestrator.sock|pid      daemon endpoint
    run/orchestrator.log           daemon stdout
"""

import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENGINE_DIR = os.environ.get("LIFESCAPE_ENGINE_DIR") or os.path.normpath(os.path.join(ROOT, "..", "lostcity", "engine"))
PACK_DIR = os.path.join(ENGINE_DIR, "data", "pack")
RUN_DIR = os.path.join(ROOT, "run")
USERS_DIR = os.path.join(RUN_DIR, "users")
LOGS_DIR = os.path.join(RUN_DIR, "logs")
SOCKET_PATH = os.path.join(RUN_DIR, "orchestrator.sock")
PID_PATH = os.path.join(RUN_DIR, "orchestrator.pid")
DAEMON_LOG = os.path.join(RUN_DIR, "orchestrator.log")


def user_paths(account):
    """(merged state, bot heartbeat, log) for one account."""
    return (
        os.path.join(USERS_DIR, account + ".json"),
        os.path.join(USERS_DIR, account + ".bot.json"),
        os.path.join(LOGS_DIR, account + ".log"),
    )


def ensure_dirs():
    for directory in (RUN_DIR, USERS_DIR, LOGS_DIR):
        os.makedirs(directory, exist_ok=True)


def read_json(path, default=None):
    try:
        with open(path) as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return default


def write_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    temporary = path + ".tmp"
    with open(temporary, "w") as handle:
        json.dump(data, handle, sort_keys=True)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, path)
