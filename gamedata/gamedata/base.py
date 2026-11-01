"""Loading of dumped JSON tables (generated, never hand-edited)."""

import json
import os

DATA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")

_table_cache = {}


def load_table(table_name):
    if table_name not in _table_cache:
        path = os.path.join(DATA_DIR, table_name + ".json")
        with open(path, encoding="utf-8") as handle:
            _table_cache[table_name] = json.load(handle)
    return _table_cache[table_name]


def clear_cache():
    _table_cache.clear()
