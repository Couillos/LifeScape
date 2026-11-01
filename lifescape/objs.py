"""Ground items: static spawns plus live drops, and the take/drop verbs."""

import os
import zipfile

from .buffer import Reader


def load_static_objs(pack_dir):
    items = []
    with zipfile.ZipFile(os.path.join(pack_dir, ".cache", "maps-server.zip")) as archive:
        for name in archive.namelist():
            if not name.startswith("o"):
                continue
            mx, mz = (int(part) for part in name[1:].split("_"))
            packet = Reader(archive.read(name))
            while packet.remaining():
                coord = packet.u16()
                x = (coord >> 6) & 0x3F
                z = coord & 0x3F
                level = (coord >> 12) & 0x3
                for _ in range(packet.u8()):
                    obj_id = packet.u16()
                    count = packet.u8()
                    items.append((mx * 64 + x, mz * 64 + z, level, obj_id, count))
    return items


def take_op(objtypes, obj_id):
    for index, name in enumerate(objtypes[obj_id]["ops"]):
        if name and name.lower() == "take":
            return index + 1
    return None


def drop_op(objtypes, obj_id):
    for index, name in enumerate(objtypes[obj_id]["iops"]):
        if name and name.lower() == "drop":
            return index + 1
    return None
