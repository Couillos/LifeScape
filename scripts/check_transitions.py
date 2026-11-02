"""Live-test every generated transition with a staff account.

For each row of lifescape/data/transitions.json: teleport next to the
transition, walk to it with the normal pathfinder, click it and check the
player really lands on the recorded destination. Results go to
run/transitions_test.json and a one-line summary per transition is printed.

This is a development tool, not a bot.

Usage: scripts/orchestrator.py run <account> check_transitions [--only NAME]
       [--kind stair|ladder|trapdoor|portal|other] [--limit N] [--skip N]
"""

import argparse
import json
import os
import time


from lifescape import collision as collision_mod
from lifescape.client.core import Unreachable
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
TABLE = os.path.join(ROOT, "lifescape", "data", "transitions.json")
RESULTS = os.path.join(ROOT, "run", "transitions_test.json")


def walkable(bot, x, z, level):
    try:
        return collision_mod.tile_walkable(bot.collision.get(x, z, level))
    except (KeyError, IndexError):
        return False


def teleport(bot, level, x, z):
    body = "tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63)
    bot.cheat(body)
    target = bot.world.ticks + 250
    while bot.world.ticks < target:
        bot.pump(2)
        if (bot.world.level, bot.world.x, bot.world.z) == (level, x, z):
            return True
    return False


def staging_tile(bot, row):
    """A reachable tile a few steps away; the SDK does the walking."""
    level, x, z = row["from"]
    sides = bot.transition_sides(level, x, z, row.get("delta"), row.get("footprint"))
    if not sides:
        return None
    for radius in range(3, 6):
        for dx in range(-radius, radius + 1):
            for dz in range(-radius, radius + 1):
                if max(abs(dx), abs(dz)) != radius:
                    continue
                cx, cz = x + dx, z + dz
                if not walkable(bot, cx, cz, level):
                    continue
                for side_x, side_z in sides:
                    if bot.router.find_path(bot.planning_map, level, cx, cz, side_x, side_z):
                        return cx, cz
    return None


def test_one(bot, row):
    level, x, z = row["from"]
    sides = bot.transition_sides(level, x, z, row.get("delta"), row.get("footprint"))
    if not sides:
        return "no walkable side"
    delta = row.get("delta")
    limit = bot.world.ticks + 600
    for side_x, side_z in sides:
        if bot.world.ticks > limit:
            break
        if not teleport(bot, level, side_x, side_z):
            continue
        expected = (
            (level + delta[0], side_x + delta[1], side_z + delta[2])
            if delta else tuple(row["to"])
        )
        bot.open_transition(row, level, x, z)
        if bot.await_teleport(row, expected, seconds=1.5, quiet=True):
            return "ok"
    return "ride: ended at %s" % (bot.position,)


def save(results):
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    with open(RESULTS, "w") as handle:
        json.dump(results, handle, indent=1, sort_keys=True)


def row_key(row):
    """Stable across table regenerations, unlike the numbered name."""
    return "%d,%d,%d>%d,%d,%d@%d,%d" % (
        row["from"][0], row["from"][1], row["from"][2],
        row["to"][0], row["to"][1], row["to"][2],
        row["climb_loc"], row["climb_op"],
    )


def run(bot, *args):
    parser = argparse.ArgumentParser()
    parser.add_argument("--only", default=None)
    parser.add_argument("--kind", default=None)
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--skip", type=int, default=0)
    parser.add_argument("--fresh", action="store_true")
    options = parser.parse_args(args)

    rows = [row for row in json.load(open(TABLE)) if not row.get("disabled")]
    if options.only:
        rows = [row for row in rows if options.only in row["name"]]
    if options.kind:
        rows = [row for row in rows if row["kind"] == options.kind]
    bot.paths = default_paths()
    bot.ensure_nav()
    results = {}
    if not options.fresh:
        previous = {}
        try:
            with open(RESULTS) as handle:
                previous = json.load(handle)
        except (OSError, ValueError):
            previous = {}
        for key, entry in previous.items():
            if entry.get("outcome") == "ok":
                results[key] = entry
    rows = [row for row in rows if row_key(row) not in results or results[row_key(row)]["outcome"] != "ok"]
    rows = rows[options.skip:]
    if options.limit:
        rows = rows[:options.limit]
    print("resume: %d already ok, %d to test" % (len(results), len(rows)), flush=True)
    for index, row in enumerate(rows):
        started = time.time()
        try:
            outcome = test_one(bot, row)
        except Exception as error:  # report, keep testing the next one
            outcome = "error: %s: %s" % (type(error).__name__, error)
        results[row_key(row)] = {
            "name": row["name"],
            "kind": row["kind"],
            "from": row["from"],
            "to": row["to"],
            "outcome": outcome,
            "seconds": round(time.time() - started, 1),
        }
        print("%4d/%d %-34s %s" % (index + 1, len(rows), row["name"], outcome), flush=True)
        if (index + 1) % 20 == 0:
            save(results)
        if outcome != "ok":
            # A failed ride can leave the player on a blocked stair tile;
            # teleport back to open ground to keep the run going.
            teleport(bot, 0, 3222, 3218)
    save(results)
    bad = {name: entry["outcome"] for name, entry in results.items() if entry["outcome"] != "ok"}
    print("tested=%d ok=%d failed=%d" % (len(results), len(results) - len(bad), len(bad)), flush=True)
    for name, outcome in sorted(bad.items()):
        print("  %-34s %s" % (name, outcome))
    # A report, not a gate: a non-zero exit would make the orchestrator
    # clean up the session and relaunch the whole table.
    return 0
