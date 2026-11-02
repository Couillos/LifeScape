"""Differential test: our router vs the server's own BFS on real maps.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/diff_routes.py [count]
"""

import json
import os
import random
import subprocess
import sys


from lifescape import collision, route

HERE = os.path.dirname(os.path.abspath(__file__))
ENGINE = os.path.normpath(os.path.join(HERE, "..", "..", "lostcity", "engine"))
PACK_DIR = os.path.join(ENGINE, "data", "pack")
TYPES = os.path.join(HERE, "..", "lifescape", "data", "gametypes.json")
PAIRS = "/tmp/opencode/route_pairs.json"
NODE_OUT = "/tmp/opencode/route_node.json"

SPAWNS = [(0, 3222, 3222), (0, 3200, 3200), (0, 3240, 3240), (0, 3222, 3250)]


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 30
    rng = random.Random(7)
    pairs = []
    for i in range(count):
        level, sx, sz = SPAWNS[i % len(SPAWNS)]
        if i == count - 1:
            pairs.append([level, sx, sz, sx + 100, sz + 100])
        else:
            pairs.append([level, sx, sz, sx + rng.randrange(-60, 61), sz + rng.randrange(-60, 61)])
    with open(PAIRS, "w") as handle:
        json.dump(pairs, handle)

    print("loading maps...", flush=True)
    world, _static, _actions = collision.load_maps(PACK_DIR, collision.load_types(TYPES))
    router = route.Router()
    print("routing in python...", flush=True)
    own = [router.find_path(world, level, sx, sz, dx, dz) for level, sx, sz, dx, dz in pairs]

    print("routing in node...", flush=True)
    subprocess.run(["npx", "tsx", "tools/lifescape-routes.mts", PAIRS, NODE_OUT], cwd=ENGINE, check=True, capture_output=True)
    expected = json.load(open(NODE_OUT))

    bad = 0
    for i, ((level, sx, sz, dx, dz), mine, theirs) in enumerate(zip(pairs, own, expected)):
        want = [(p >> 28 & 0x3, (p >> 14) & 0x3FFF, p & 0x3FFF) for p in theirs["path"]]
        if mine != want:
            bad += 1
            print("MISMATCH %d (%d,%d)->(%d,%d)" % (i, sx, sz, dx, dz))
            print("  python: %s" % (mine,))
            print("  node:   %s" % (want,))

    xs = [p[1] for p in pairs] + [p[3] for p in pairs]
    zs = [p[2] for p in pairs] + [p[4] for p in pairs]
    box = (min(xs) - 70, min(zs) - 70, max(xs) + 70, max(zs) + 70, 0, "/tmp/opencode/route_flags.json")
    subprocess.run(["npx", "tsx", "tools/lifescape-flags.mts"] + [str(v) for v in box], cwd=ENGINE, check=True, capture_output=True)
    for x, z, flag in json.load(open("/tmp/opencode/route_flags.json")):
        if world.get(x, z, 0) != (flag & 0xFFFFFFFF):
            bad += 1
            print("FLAG MISMATCH (%d,%d,0): py=%08x node=%08x" % (x, z, world.get(x, z, 0), flag & 0xFFFFFFFF))
            break
    print("pairs=%d mismatches=%d" % (count, bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
