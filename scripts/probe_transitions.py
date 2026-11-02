"""Fast side probe for transitions that the batched check reports as failing.

For each transition: teleport onto every orthogonal tile around the loc
footprint, click it, and record whether the player actually moved. No
walking, no door chasing: this only answers "from which tile does the
server accept this click, and where does it land".

Usage: ../lostcity/.pypy/bin/pypy3 scripts/probe_transitions.py [--name NAME]
"""

import argparse
import json
import os
import sys
import time


from lifescape.client import Bot
from lifescape.connection import default_paths

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
TABLE = os.path.join(ROOT, "lifescape", "data", "transitions.json")
RESULTS = os.path.join(ROOT, "run", "transitions_test.json")


def settle(bot, seconds=0.15):
    deadline = time.time() + seconds
    while time.time() < deadline:
        bot.pump(1)


def teleport(bot, level, x, z):
    bot.cheat("tele %d,%d,%d,%d,%d" % (level, x >> 6, z >> 6, x & 63, z & 63))
    settle(bot)
    return (bot.world.level, bot.world.x, bot.world.z) == (level, x, z)


def around(footprint):
    inside = {(x, z) for x, z in footprint}
    tiles = []
    for x, z in inside:
        for side in ((x, z - 1), (x + 1, z), (x, z + 1), (x - 1, z)):
            if side not in inside and side not in tiles:
                tiles.append(side)
    return tiles


def probe(bot, row):
    level, x, z = row["from"]
    expected = (
        (level + row["delta"][0], None, None) if row.get("delta") else tuple(row["to"])
    )
    working = []
    for side_x, side_z in around(row["footprint"]):
        if not teleport(bot, level, side_x, side_z):
            continue
        want = (
            (level + row["delta"][0], side_x + row["delta"][1], side_z + row["delta"][2])
            if row.get("delta") else expected
        )
        bot.op_loc(row["climb_op"], x, z, row["climb_loc"])
        deadline = time.time() + 1.5
        landed = None
        while time.time() < deadline:
            bot.pump(1)
            if (bot.world.level, bot.world.x, bot.world.z) != (level, side_x, side_z):
                landed = bot.position
                break
        if landed is not None:
            working.append(((side_x, side_z), landed, landed == want))
            if bot.world.level != level:
                teleport(bot, level, side_x, side_z)
    return working


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("account", nargs="?", default="admin")
    parser.add_argument("--name", default=None)
    parser.add_argument("--all", action="store_true")
    options = parser.parse_args()

    rows = {row["name"]: row for row in json.load(open(TABLE))}
    results = json.load(open(RESULTS)) if os.path.exists(RESULTS) else {}
    table = {
        "%d,%d,%d>%d,%d,%d@%d,%d" % (
            row["from"][0], row["from"][1], row["from"][2],
            row["to"][0], row["to"][1], row["to"][2], row["climb_loc"], row["climb_op"],
        ): row for row in rows.values()
    }
    targets = [entry["name"] for entry in results.values() if entry["outcome"] != "ok"]
    if options.all:
        targets = sorted(rows)
    if options.name:
        targets = [name for name in targets if options.name in name]

    bot = Bot()
    bot.login(options.account, options.account)
    bot.paths = default_paths()
    bot.ensure_nav()
    bot.pump(3)
    try:
        for name in sorted(targets):
            row = rows.get(name)
            if row is None:
                print("%-34s (absent de la table)" % name, flush=True)
                continue
            working = probe(bot, row)
            if working:
                for side, landed, exact in working:
                    print("%-34s depuis %-12s -> %-18s %s" % (
                        name, str(side), str(landed), "ok" if exact else "MAUVAIS")
                    , flush=True)
            else:
                print("%-34s aucun cote n'accepte le clic" % name, flush=True)
    finally:
        bot.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
