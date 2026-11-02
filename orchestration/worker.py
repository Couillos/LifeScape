"""Internal: run one bot script under the orchestrator daemon.

Usage (daemon only):

    python -m orchestration.worker <account> <script> [args]

Sets ``LIFESCAPE_*`` for the SDK heartbeat and maps a random-event abort
(SystemExit carrying "random event") to exit code 75, which tells the
daemon to trigger the matching rescue before relaunching the bot.
"""

import os
import runpy
import sys

from .registry import script_path

RANDOM_EVENT_CODE = 75

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def ensure_project_paths():
    """Put the project on sys.path for the supervised script.

    Every bot, probe and test runs through this worker (the only entry
    point), so scripts import ``lifescape``, ``scripts`` and ``gamedata``
    directly with no per-file ``sys.path`` boilerplate. The inner
    ``gamedata/`` dir comes first: ``import gamedata`` must resolve to
    the generated tables, shadowing the outer container.
    """
    for entry in (PROJECT_ROOT, os.path.join(PROJECT_ROOT, "gamedata")):
        if entry not in sys.path:
            sys.path.insert(0, entry)


def main():
    if len(sys.argv) < 3:
        print("usage: -m orchestration.worker <account> <script> [args]", file=sys.stderr)
        return 2
    account, script, *args = sys.argv[1:]
    path = script_path(script)
    os.environ.setdefault("LIFESCAPE_USER", account)
    sys.argv = [path, account] + args
    ensure_project_paths()
    try:
        runpy.run_path(path, run_name="__main__")
    except SystemExit as stop:
        code = stop.code
        if isinstance(code, str):
            if "random event" in code:
                return RANDOM_EVENT_CODE
            return 1
        return int(code or 0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
