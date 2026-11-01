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


def main():
    if len(sys.argv) < 3:
        print("usage: -m orchestration.worker <account> <script> [args]", file=sys.stderr)
        return 2
    account, script, *args = sys.argv[1:]
    path = script_path(script)
    os.environ.setdefault("LIFESCAPE_USER", account)
    sys.argv = [path, account] + args
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
