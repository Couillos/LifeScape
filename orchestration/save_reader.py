"""Player save reads owned by the orchestrator: bot scripts never touch saves."""

import os

from . import holdings


def look_path(path):
    """(gender, kits, colors) from a save path, or None if unreadable."""
    try:
        with open(path, "rb") as handle:
            data = handle.read()
    except OSError:
        return None
    if len(data) < 22 or int.from_bytes(data[0:2], "big") != 0x2004:
        return None
    kits = [byte if byte != 255 else -1 for byte in data[9:16]]
    colors = list(data[16:21])
    return data[21], kits, colors


def look(username):
    return look_path(os.path.join(holdings.SAVE_DIR, username + ".sav"))
