"""Generate the F2P transition table from the server scripts.

A transition is one click that moves the player to another tile: stairs,
ladders, trapdoors, manholes, ropes and portals. The source coordinate and
the destination come from the ``oploc`` handlers in lostcity/content; the
loc ids come from the map placements and the cache loc types.

The F2P filter uses the zone at level 0, like the engine's isFreeToPlay()
(level is ignored, an upper floor of a free zone is free too).

This is a development tool, not a bot.
"""

import json
import os
import re
import zipfile


from lifescape import collision
from lifescape.buffer import Reader
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
CONTENT = os.path.join(ROOT, "..", "lostcity", "content")
OUT = os.path.join(ROOT, "lifescape", "data", "transitions.json")

ABSOLUTE = re.compile(r"(?<![0-9_])(\d+)_(\d+)_(\d+)_(\d+)_(\d+)(?![0-9_])")
# movecoord(coord(), ...) is relative to the *player*: the script teleports
# whoever clicked, from wherever they stand next to the ladder.
PLAYER_MOVE = re.compile(
    r"movecoord\(\s*coord(?:\(\))?\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*\)"
)
# movecoord(loc_coord(), ...) is relative to the loc itself.
LOC_MOVE = re.compile(
    r"movecoord\(\s*loc_coord(?:\(\))?\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*\)"
)
TELEPORT = re.compile(r"(?:p_telejump|p_teleport|climb_ladder|tele_jump|teleport)\(\s*([^,)]+)")
OPENED = re.compile(r"loc_change\(\s*([a-z0-9_]+)")
HANDLER = re.compile(r"oploc([1-5]),([a-z0-9_]+)$")

# Verified live: the server refuses the click from every side, because the
# handler itself gates it. Kept in the table for documentation, skipped by
# the planner and the live check.
DISABLED = {
    ((0, 3046, 3483), (1, 3046, 3483)): "Prayer Guild ladder: members of the order only (%prayer_guild)",
    ((0, 3057, 3483), (1, 3057, 3483)): "Prayer Guild ladder: members of the order only (%prayer_guild)",
    ((0, 3111, 9526), (0, 3111, 3126)): "Tutorial Island ladder: tutorial progress gate",
    ((0, 3088, 3119), (0, 3088, 9519)): "Tutorial Island ladder: tutorial progress gate",
    ((0, 3302, 3116), (0, 3302, 3119)): "Shantay Pass gate: members only, not a level change",
}


def f2p_zones():
    """Zone keys (x_zone, z_zone) of the free-to-play map, level ignored."""
    found = set()
    with open(os.path.join(CONTENT, "maps", "free2play.csv")) as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith("//"):
                continue
            parts = line.split("_")
            if len(parts) != 5:
                continue
            _level, mx, mz, lx, lz = (int(part) for part in parts)
            found.add((mx * 8 + (lx >> 3), mz * 8 + (lz >> 3)))
    return found


def map_packets(pack_dir):
    zipped = os.path.join(pack_dir, ".cache", "maps-server.zip")
    if os.path.exists(zipped):
        with zipfile.ZipFile(zipped) as archive:
            for name in sorted(archive.namelist()):
                if not name.startswith("l"):
                    continue
                mx, mz = (int(part) for part in name[1:].split("_"))
                yield mx << 6, mz << 6, archive.read(name)
        return
    maps_dir = os.path.join(pack_dir, "server", "maps")
    for name in sorted(os.listdir(maps_dir)):
        if not name.startswith("l"):
            continue
        mx, mz = (int(part) for part in name[1:].split("_"))
        with open(os.path.join(maps_dir, name), "rb") as handle:
            yield mx << 6, mz << 6, handle.read()


def placements(pack_dir, types, zones):
    """Every static loc on a free-to-play tile, duplicates included."""
    for square_x, square_z, packet in map_packets(pack_dir):
        reader = Reader(packet)
        loc_id = -1
        offset = reader.smarts()
        while offset != 0:
            loc_id += offset
            coord = 0
            coord_offset = reader.smarts()
            while coord_offset != 0:
                coord += coord_offset - 1
                z = coord & 0x3F
                x = (coord >> 6) & 0x3F
                level = (coord >> 12) & 0x3
                info = reader.u8()
                coord_offset = reader.smarts()
                absolute_x = square_x + x
                absolute_z = square_z + z
                if (absolute_x >> 3, absolute_z >> 3) in zones:
                    yield absolute_x, absolute_z, level, loc_id, info >> 2, info & 0x3
            offset = reader.smarts()


def handlers(root):
    """{(loc name, op): [body]} for every [oplocN,name] block."""
    found = {}
    for directory, _subdirs, files in os.walk(os.path.join(root, "scripts")):
        for name in files:
            if not name.endswith(".rs2"):
                continue
            path = os.path.join(directory, name)
            for header, body in script_blocks(path):
                match = HANDLER.match(header)
                if match:
                    found.setdefault((match.group(2), int(match.group(1))), []).append((body, path))
    return found


def script_blocks(path):
    header = None
    body = []
    with open(path) as handle:
        for line in handle:
            stripped = line.strip()
            if stripped.startswith("[") and stripped.endswith("]"):
                if header:
                    yield header, "\n".join(body)
                header = stripped[1:-1]
                body = []
            elif header:
                body.append(line)
    if header:
        yield header, "\n".join(body)


def packed(level, x, z):
    return "%d_%d_%d_%d_%d" % (level, x >> 6, z >> 6, x & 0x3F, z & 0x3F)


def unpack(text):
    level, mx, mz, lx, lz = (int(part) for part in text.split("_"))
    return level, mx * 64 + lx, mz * 64 + lz


def angle_body(body, angle):
    """Body of the switch_int(loc_angle) case for this placement."""
    for line in body.splitlines():
        stripped = line.strip()
        match = re.match(r"case\s+(\d+|default)\s*:\s*(.*)$", stripped)
        if match and match.group(1) in (str(angle), "default"):
            return match.group(2)
    return None


def _switch_tail(body):
    """Code after the first switch_coord block (its generic path).

    Some handlers only special-case one coord and let the rest fall
    through to shared code below the switch (e.g. laddertop_directional:
    the Black Knights ladder has its own case, every other laddertop
    shares the climb below).
    """
    lines = body.splitlines()
    for index, line in enumerate(lines):
        if "switch_coord" not in line:
            continue
        depth = 0
        started = False
        for rest in range(index, len(lines)):
            for char in lines[rest]:
                if char == "{":
                    depth += 1
                    started = True
                elif char == "}":
                    depth -= 1
                    if started and depth == 0:
                        return "\n".join(lines[rest + 1:])
        return None
    return None


def case_body(body, frm):
    """Body of the switch_coord case for ``frm``, or the default case.

    A case arm is usually one line, but some span several (e.g. the Black
    Knights ladder, whose case holds a climb plus an aggro call): when the
    case line itself carries nothing, gather the following lines up to the
    next case or the closing brace.
    """
    key = packed(*frm)
    fallback = None
    lines = body.splitlines()
    index = 0
    while index < len(lines):
        stripped = lines[index].strip()
        match = re.match(r"case\s+([0-9_]+|default)\s*:\s*(.*)$", stripped)
        index += 1
        if not match:
            continue
        chunk = match.group(2)
        if not chunk.strip() or chunk.strip().startswith("//"):
            extra = []
            while index < len(lines):
                ahead = lines[index].strip()
                if re.match(r"case\s+([0-9_]+|default)\s*:", ahead) or ahead == "}":
                    break
                extra.append(lines[index])
                index += 1
            chunk = chunk + "\n" + "\n".join(extra)
        if match.group(1) == key:
            return chunk
        if match.group(1) == "default" and fallback is None:
            fallback = chunk
    if fallback is None:
        fallback = _switch_tail(body)
    return fallback


def moved(offsets, frm):
    """movecoord args are (x offset, level offset, z offset)."""
    dx, dlevel, dz = (int(value) for value in offsets)
    return (frm[0] + dlevel, frm[1] + dx, frm[2] + dz), (dlevel, dx, dz)


def destination(text, frm):
    """(destination, delta) for the first teleport in ``text``.

    Only the statement that actually moves the player counts: a handler can
    also move npcs or objects with movecoord(coord, ...) later on.
    ``delta`` is set when the script uses the player's own tile, so the
    arrival depends on where the bot stood when it clicked.
    """
    for line in text.splitlines():
        if not TELEPORT.search(line):
            continue
        direct = ABSOLUTE.search(line)
        if direct:
            return unpack(direct.group(0)), None
        found = PLAYER_MOVE.search(line)
        if found:
            return moved(found.groups(), frm)
        found = LOC_MOVE.search(line)
        if found:
            target, _delta = moved(found.groups(), frm)
            return target, None
    return None, None


def resolved(name, op, handlers_map, frm, angle=0):
    bodies = handlers_map.get((name, op))
    if not bodies:
        return None
    body = "\n".join(entry[0] for entry in bodies)
    if "switch_coord" in body:
        body = case_body(body, frm)
    elif "switch_int" in body and "loc_angle" in body:
        body = angle_body(body, angle)
    if body is None:
        return None
    return destination(body, frm)


def opened_to(name, op, handlers_map, frm, angle=0):
    """(opened name, op, (destination, delta)) when the click opens a trapdoor."""
    bodies = handlers_map.get((name, op))
    if not bodies:
        return None
    body = "\n".join(entry[0] for entry in bodies)
    if "switch_coord" in body:
        body = case_body(body, frm)
    elif "switch_int" in body and "loc_angle" in body:
        body = angle_body(body, angle)
    if body is None:
        return None
    match = OPENED.search(body)
    if not match:
        return None
    opened = match.group(1)
    for next_op in sorted(op for (candidate, op) in handlers_map if candidate == opened):
        found = resolved(opened, next_op, handlers_map, frm, angle)
        if found is not None and found[0] is not None:
            return opened, next_op, found
    return None


def footprint_tiles(types, level, x, z, loc_id, shape, angle):
    """Every tile the loc covers: a 2x2 staircase is entered from any edge."""
    entry = types["loc"][loc_id] if 0 <= loc_id < len(types["loc"]) else None
    if entry is None or shape <= 3:
        return [[x, z]]
    probe = collision.CollisionMap()
    collision.change_loc_collision(
        probe, shape, angle, entry["blockrange"], entry["length"], entry["width"],
        entry["active"], x, z, level, True,
    )
    tiles = []
    for key, zone in probe.zones.items():
        base_x = (key & 0x7FF) << 3
        base_z = ((key >> 11) & 0x7FF) << 3
        for index, flags in enumerate(zone):
            if flags:
                tiles.append([base_x + (index & 0x7), base_z + ((index >> 3) & 0x7)])
    return tiles or [[x, z]]


def kind_of(name):
    if "stair" in name:
        return "stair"
    if "ladder" in name or "rope" in name or "climb" in name:
        return "ladder"
    if "trapdoor" in name or "manhole" in name or "cellar" in name:
        return "trapdoor"
    if "portal" in name or "volcano" in name or "cave" in name:
        return "portal"
    return "other"


def main():
    paths = default_paths()
    types = collision.load_types(paths["types"])
    zones = f2p_zones()
    handlers_map = handlers(CONTENT)

    by_name = {}
    for x, z, level, loc_id, shape, angle in placements(paths["pack_dir"], types, zones):
        name = (types["loc"][loc_id].get("name") or "").lower()
        if not name:
            continue
        by_name.setdefault(name, []).append((x, z, level, loc_id, shape, angle))

    id_of_name = {}
    for name, spots in by_name.items():
        id_of_name.setdefault(name, spots[0][3])
    for loc_id, entry in enumerate(types["loc"]):
        name = (entry.get("name") or "").lower()
        if name:
            id_of_name.setdefault(name, loc_id)

    rows = []
    seen = set()
    skipped = []
    for name, spots in sorted(by_name.items()):
        ops = sorted(op for (candidate, op) in handlers_map if candidate == name)
        if not ops:
            continue
        for x, z, level, loc_id, shape, angle in spots:
            frm = (level, x, z)
            for op in ops:
                opened = opened_to(name, op, handlers_map, frm, angle)
                if opened is not None:
                    opened_name, opened_op, found = opened
                    target, delta = found
                    if target == frm:
                        continue
                    climb_loc = id_of_name.get(opened_name)
                    climb_op = opened_op
                else:
                    found = resolved(name, op, handlers_map, frm, angle)
                    if found is None or found[0] is None:
                        continue
                    target, delta = found
                    if target == frm:
                        continue
                    climb_loc = loc_id
                    climb_op = op
                if target[0] not in (0, 1, 2, 3):
                    continue
                if target[0] == frm[0] and abs(target[1] - x) + abs(target[2] - z) <= 2:
                    # doors, gates and levers: the same-level router handles them
                    skipped.append((name, op, frm, target))
                    continue
                key = (frm, target, climb_loc, climb_op)
                if key in seen:
                    continue
                seen.add(key)
                rows.append({
                    "name": "%s_%d" % (name, len(rows)),
                    "kind": kind_of(name),
                    "from": [frm[0], frm[1], frm[2]],
                    "to": [target[0], target[1], target[2]],
                    "loc": loc_id,
                    "climb_loc": climb_loc,
                    "climb_op": climb_op,
                    "open_to": id_of_name.get(opened_name) if opened is not None else None,
                    "delta": list(delta) if delta is not None else None,
                    "footprint": footprint_tiles(types, level, x, z, loc_id, shape, angle),
                    "disabled": DISABLED.get((tuple(frm), tuple(target))),
                })

    with open(OUT, "w") as handle:
        json.dump(rows, handle, indent=1, sort_keys=True)
    print("transitions=%d skipped=%d -> %s" % (len(rows), len(skipped), OUT))
    for name, op, frm, target in skipped:
        print("  skipped %s op%d %s -> %s" % (name, op, frm, target))


if __name__ == "__main__":
    main()
