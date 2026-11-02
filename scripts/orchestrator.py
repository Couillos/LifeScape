"""CLI + daemon entry point for the bot orchestrator.

    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py open olivia_king
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py run olivia_king fishing 1
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py status
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py show olivia_king
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py logs olivia_king 40
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py call olivia_king inventory
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py unload olivia_king
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py close olivia_king
    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py test

The daemon itself (docker compose service, or manual):

    ../lostcity/.pypy/bin/pypy3 scripts/orchestrator.py daemon
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from orchestration.cli import main

if __name__ == "__main__":
    sys.exit(main(sys.argv))
