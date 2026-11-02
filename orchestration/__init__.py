"""Bot orchestrator: process supervision, per-account state and web view."""

import os
import sys

_GAMEDATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gamedata")
if _GAMEDATA not in sys.path:
    sys.path.insert(0, _GAMEDATA)
