"""Stands de banque verifies : ou se mettre pour utiliser le booth.

Table generee par scripts/dev/dump_bank_stands.py (flood + ReachStrategy
sur la collision statique, portes ouvertes). Le script marche sur une
case libre, le SDK ouvre le booth depuis le stand.
"""

import json
import os

_TABLE = None


def table():
    global _TABLE
    if _TABLE is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data", "bank_stands.json")
        with open(path) as handle:
            _TABLE = json.load(handle)
    return _TABLE


def stand_for(bank_x, bank_z, level=0):
    """(x, z) du stand du booth donne, ou None si inconnu."""
    best = None
    for entry in table():
        for booth_x, booth_z in entry.get("booths", []):
            dist = abs(booth_x - bank_x) + abs(booth_z - bank_z)
            if best is None or dist < best[0]:
                stand = entry.get("stand")
                if stand is not None and stand[2] == level:
                    best = (dist, (stand[0], stand[1]))
    return best[1] if best is not None else None
