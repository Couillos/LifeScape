"""Shim so ``import gamedata`` works with only the repo root on sys.path.

The real tables live in ``gamedata/gamedata/``. When the inner parent is
on sys.path the real package wins and this file never loads; in a bare
``python -m unittest discover tests`` only the root is on sys.path, so
this shim exposes the inner API instead of an empty namespace package.
"""
import os as _os

__path__.append(_os.path.join(_os.path.dirname(__file__), "gamedata"))

from gamedata.gamedata import *  # noqa: F401,F403
