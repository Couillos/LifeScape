"""Zone naming from the world map labels the server ships, plus the
random-event instances the labels never cover.

labels.dat is bundled in engine/data/pack/mapview/worldmap.jag
(count p2; per label: pjstr name (0x0A-terminated) + p2 x + p2 z + p1 type).
Nothing is invented: the nearest shipped label wins, or none. The type
column (0=place, 1=town, 2=kingdom) travels with the name so callers can
know how local the label is; the label itself has no radius, so the
distance to its centre is the only honesty available.

Random events teleport off the labelled map (maze, mime): their boxes are
the ones the rescue logic already uses, named after the event.
"""

import bz2
import os

# jag members are bzip2 streams missing their "BZh1" magic header
MAGIC = b"BZh1"

# Per-type attraction radius, in tiles: local places are only claimed close
# up, towns cover their surroundings, regions are fallback only.
TYPE_RADIUS = {0: 60, 1: 120, 2: 600}

# Random-event instances, off the labelled map: ((x0, x1), (z0, z1)).
# Same boxes the rescue logic uses (scripts/randomevents.py).
EVENT_AREAS = {
    "maze": ((2880, 2944), (4544, 4608)),
    "mime": ((1984, 2048), (4736, 4800)),
}


def event_area(x, z):
    """Event name when (x, z) sits in a random-event instance, else None."""
    for name, ((x0, x1), (z0, z1)) in EVENT_AREAS.items():
        if x0 <= x < x1 and z0 <= z < z1:
            return name
    return None


def _hash(name):
    total = 0
    for char in name.upper():
        total = (total * 61 + ord(char) - 32) & 0xFFFFFFFF
    return total - 0x100000000 if total >= 0x80000000 else total


def _member(body, name):
    count = int.from_bytes(body[0:2], "big")
    pos = 2 + count * 10
    found = None
    for index in range(count):
        base = 2 + index * 10
        file_hash = int.from_bytes(body[base:base + 4], "big", signed=True)
        chunk = body[pos:pos + int.from_bytes(body[base + 7:base + 10], "big")]
        pos += len(chunk)
        if found is None and file_hash == _hash(name):
            if int.from_bytes(body[base + 4:base + 7], "big") != len(chunk):
                chunk = bz2.decompress(MAGIC + chunk)
            found = chunk
    return found


def load_labels(pack_dir):
    """All (name, x, z, type) labels, world coordinates; [] if jag missing."""
    path = os.path.join(pack_dir, "mapview", "worldmap.jag")
    with open(path, "rb") as handle:
        body = handle.read()
    unpacked_whole = int.from_bytes(body[0:3], "big")
    packed_whole = int.from_bytes(body[3:6], "big")
    body = body[6:]
    if unpacked_whole != packed_whole:
        body = bz2.decompress(MAGIC + body[6:])
    data = _member(body, "labels.dat")
    if data is None:
        return []
    labels = []
    pos = 2
    for _ in range(int.from_bytes(data[0:2], "big")):
        end = data.index(b"\x0a", pos)
        name = data[pos:end].decode("ascii", "replace")
        pos = end + 1
        x = int.from_bytes(data[pos:pos + 2], "big")
        z = int.from_bytes(data[pos + 2:pos + 4], "big")
        kind = data[pos + 4]
        pos += 5
        labels.append((name, x, z, kind))
    return labels


def nearest(labels, x, z):
    """(name, distance, type) of the closest honest label for a tile.

    Local places (radius 60) and towns (radius 200) compete by distance,
    so a courtyard shows "Palace" while the mine east of Varrock (no local
    label) shows "Varrock". Regions (radius 600) are fallback only, so a
    region centre never hides the town next door. None when nothing claims
    the tile.
    """
    place = None
    for name, lx, lz, kind in labels:
        if kind == 2:
            continue
        distance = ((lx - x) ** 2 + (lz - z) ** 2) ** 0.5
        if distance <= TYPE_RADIUS.get(kind, 0) and (place is None or distance < place[1]):
            place = (name, distance, kind)
    if place is not None:
        return place
    region = None
    for name, lx, lz, kind in labels:
        if kind != 2:
            continue
        distance = ((lx - x) ** 2 + (lz - z) ** 2) ** 0.5
        if distance <= TYPE_RADIUS.get(kind, 0) and (region is None or distance < region[1]):
            region = (name, distance, kind)
    return region
