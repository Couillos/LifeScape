"""Live-test free-to-play doors, one instance per distinct loc.

Every openable wall in the F2P map shares its loc type with dozens of
others, so opening one instance of each type proves the click sequence and
the routing through the opening. For each sampled door: teleport beside it,
open it, walk to a tile on the other side and check we got there.

This is a development tool, not a bot.

Usage: scripts/orchestrator.py run <account> check_doors [--limit N] [--skip N] [--only ID]
"""

import argparse
import json
import os
import time


from lifescape import collision as collision_mod
from lifescape.client.core import Unreachable
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
RESULTS = os.path.join(ROOT, "run", "doors_test.json")
CONTENT = os.path.join(ROOT, "..", "lostcity", "content")


def f2p_zones():
    """Free zones from the server's map filter; everything when absent.

    The bots container only mounts the engine, not content/, so the CSV is
    often missing there; the tool still samples one door per loc id.
    """
    zones = set()
    path = os.path.join(CONTENT, "maps", "free2play.csv")
    if not os.path.exists(path):
        print("no free2play.csv at %s, sampling every zone" % (path,), flush=True)
        return None
    with open(path) as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith("//"):
                continue
            parts = line.split("_")
            if len(parts) != 5:
                continue
            _level, mx, mz, lx, lz = (int(part) for part in parts)
            zones.add((mx * 8 + (lx >> 3), mz * 8 + (lz >> 3)))
    return zones


def teleport(bot, level, x, z):
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    target = bot.world.ticks + 500
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            return True
    return False


def walkable(bot, x, z, level):
    try:
        return collision_mod.tile_walkable(bot.collision.get(x, z, level))
    except (KeyError, IndexError):
        return False


def sides(bot, spot):
    """Two walkable tiles on opposite sides of the door, closest first."""
    tiles = [tile for tile in bot.door_footprints[id(spot)]]
    if not tiles:
        return None
    centre_x = sum(tile[0] for tile in tiles) / len(tiles)
    centre_z = sum(tile[1] for tile in tiles) / len(tiles)
    level = spot[2]
    neighbours = set()
    for tile_x, tile_z, _level in tiles:
        for dx in (-1, 0, 1):
            for dz in (-1, 0, 1):
                if (dx, dz) == (0, 0):
                    continue
                nx, nz = tile_x + dx, tile_z + dz
                if (nx, nz, level) in bot.door_footprints[id(spot)]:
                    continue
                if walkable(bot, nx, nz, level):
                    neighbours.add((nx, nz))
    best = None
    for ax, az in neighbours:
        for bx, bz in neighbours:
            if (ax, az) == (bx, bz):
                continue
            dot = (ax - centre_x) * (bx - centre_x) + (az - centre_z) * (bz - centre_z)
            if dot >= 0:
                continue
            spread = abs(ax - bx) + abs(az - bz)
            if best is None or spread > best[0]:
                best = (spread, (ax, az), (bx, bz))
    return (best[1], best[2]) if best else None


def sample_doors(bot):
    zones = f2p_zones()
    seen = {}
    for spot in bot.wall_doors:
        if zones is not None and (spot[0] >> 3, spot[1] >> 3) not in zones:
            continue
        if sides(bot, spot) is None:
            continue
        seen.setdefault(spot[9], []).append(spot)
    return [spots[0] for _loc_id, spots in sorted(seen.items())]


def test_one(bot, spot):
    level = spot[2]
    pair = sides(bot, spot)
    if pair is None:
        return "no open side"
    (ax, az), (bx, bz) = pair
    if not teleport(bot, level, ax, az):
        return "teleport failed"
    if not bot.open_door(spot[0], spot[1], timeout=1000):
        return "door did not open"
    try:
        bot.walk_to(bx, bz, level, timeout=1250)
    except (Unreachable, TimeoutError) as error:
        return "walk through: %s" % error
    if (bot.world.x, bot.world.z) != (bx, bz):
        return "stopped at %s" % (bot.position,)
    return "ok"


def run(bot, *args):
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--skip", type=int, default=0)
    parser.add_argument("--only", default=None)
    options = parser.parse_args(args)

    bot.paths = default_paths()
    bot.ensure_nav()
    doors = sample_doors(bot)
    if options.only:
        doors = [spot for spot in doors if str(spot[9]) == options.only]
    doors = doors[options.skip:]
    if options.limit:
        doors = doors[:options.limit]

    results = {}
    for index, spot in enumerate(doors):
        name = bot.loc_name_at(spot[0], spot[1], spot[2]) or ""
        label = "%d@%d,%d,%d" % (spot[9], spot[0], spot[1], spot[2])
        started = time.time()
        try:
            outcome = test_one(bot, spot)
        except Exception as error:  # report, keep testing the next one
            outcome = "error: %s: %s" % (type(error).__name__, error)
        results[label] = {
            "loc": spot[9],
            "name": name,
            "kind": "door",
            "from": [spot[2], spot[0], spot[1]],
            "outcome": outcome,
            "seconds": round(time.time() - started, 1),
        }
        print("%4d/%d %-30s %-12s %s" % (index + 1, len(doors), label, name, outcome), flush=True)
        if outcome != "ok":
            teleport(bot, 0, 3222, 3218)

    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    with open(RESULTS, "w") as handle:
        json.dump(results, handle, indent=1, sort_keys=True)

    bad = {key: entry["outcome"] for key, entry in results.items() if entry["outcome"] != "ok"}
    print("doors=%d ok=%d failed=%d" % (len(results), len(results) - len(bad), len(bad)), flush=True)
    for key, outcome in sorted(bad.items()):
        print("  %-30s %s" % (key, outcome))
    return 0
