"""Read a player save file: offline state the live client never receives.

Quest completion varps and the bank are saved but not streamed, so a quest
requirement check reads them from the engine save. Positions, stats,
inventories and varps are extracted; unknown or corrupt saves return {}.
"""

import os
import struct

from .buffer import Reader

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def default_save_dir():
    return os.environ.get("LIFESCAPE_SAVE_DIR") or os.path.join(
        os.environ.get("LIFESCAPE_ENGINE_DIR") or os.path.join(PROJECT_ROOT, "..", "lostcity", "engine"),
        "data",
        "players",
        "main",
    )


def account_path(account, save_dir=None):
    directory = save_dir or default_save_dir()
    return os.path.join(directory, account.lower() + ".sav")


def load_account(account, save_dir=None):
    return parse(account_path(account, save_dir))


def _inventory_category(size):
    if size == 28:
        return "inv"
    if size == 14:
        return "worn"
    if size >= 100:
        return "bank"
    return None


def parse(path):
    try:
        with open(path, "rb") as handle:
            data = handle.read()
    except OSError:
        return {}
    if len(data) < 24:
        return {}
    try:
        reader = Reader(data)
        if reader.u16() != 0x2004:
            return {}
        version = reader.u16()
        if version > 7:
            return {}
        reader.pos = 4
        x = reader.u16()
        z = reader.u16()
        level = reader.u8()
        reader.raw(7)  # body
        reader.raw(5)  # colours
        reader.u8()  # gender
        reader.u16()  # run energy
        if version >= 2:
            reader.i32()  # playtime
        else:
            reader.u16()
        stats = {}
        for index in range(21):
            experience = reader.i32()
            stats[index] = (reader.u8(), experience)
        varps = {}
        varp_count = reader.u16()
        for _ in range(varp_count):
            if version >= 7:
                varp_id = reader.u16()
                varps[varp_id] = reader.varint()
            else:
                reader.i32()  # no ids in older saves: cannot map them
        holdings = {"inv": {}, "worn": {}, "bank": {}}
        inv_count = reader.u8()
        for _ in range(inv_count):
            reader.u16()  # inv type
            size = reader.u16() if version >= 5 else 0
            category = _inventory_category(size)
            for _slot in range(size):
                raw = reader.u16()
                if raw == 0:
                    continue
                obj_id = raw - 1
                count = reader.u8()
                if count == 255:
                    count = reader.i32()
                if category and count > 0:
                    bucket = holdings[category]
                    bucket[obj_id] = bucket.get(obj_id, 0) + count
        return {
            "x": x,
            "z": z,
            "level": level,
            "stats": stats,
            "varps": varps,
            "inv": holdings["inv"],
            "worn": holdings["worn"],
            "bank": holdings["bank"],
        }
    except (IndexError, struct.error):
        return {}
