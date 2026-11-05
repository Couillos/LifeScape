"""Unit-test package bootstrap: make `python -m unittest discover tests`
work from the repo root, with or without the orchestrator.

`import gamedata` must resolve to the generated tables in
``gamedata/gamedata/`` (they shadow the outer container by design, like
the session worker sets up). Insert both entries at the front so a bare
discover run sees the same layout as the supervised ``tests`` job.
"""

import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
for _entry in (_ROOT, os.path.join(_ROOT, "gamedata")):
    if _entry not in sys.path:
        sys.path.insert(0, _entry)
