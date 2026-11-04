"""Run the unit test suite as an orchestrator job.

The orchestrator launches this on the ephemeral ``tests`` account (no game
session): the log and exit code are visible through ``orchestrator.py logs
tests`` / the dashboard. Returns 0 when every test passes, 1 otherwise.

Usage: scripts/orchestrator.py test [pattern]
"""

import os
import unittest

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


def run(bot, *args):
    pattern = args[0] if args else "test_*.py"
    suite = unittest.defaultTestLoader.discover(os.path.join(ROOT, "tests"), pattern=pattern, top_level_dir=ROOT)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1
