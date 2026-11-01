"""Run the unit test suite as an orchestrator job.

The orchestrator launches this like any bot (account ``tests``): the log
and exit code are visible through ``orchestrator.py logs tests`` / the
dashboard, and a failure never leaves a game session behind.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/run_tests.py [pattern]
"""

import os
import sys
import unittest

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")


def main():
    pattern = sys.argv[2] if len(sys.argv) > 2 else "test_*.py"
    suite = unittest.defaultTestLoader.discover(os.path.join(ROOT, "tests"), pattern=pattern, top_level_dir=ROOT)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
