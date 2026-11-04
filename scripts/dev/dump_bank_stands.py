"""Stand verifie par banque F2P : ou se mettre pour utiliser le booth.

Pour chaque grappe de booths : un flood BFS sur la carte portes
ouvertes, puis la tuile marchable la plus proche du centroide qui est a
portee d'interaction d'un booth (ReachStrategy, comme le serveur).
Sortie : lifescape/data/bank_stands.json. A regenerer si la map change.

Usage: ../lostcity/.pypy/bin/pypy3 scripts/dev/dump_bank_stands.py
"""

import json
import os
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
sys.path.insert(0, ROOT)

OUT = os.path.join(ROOT, "lifescape", "data", "bank_stands.json")

import gamedata as knowledge

from lifescape import collision as collision_mod
from lifescape import doors as doors_mod
from lifescape import f2p as f2p_mod
from lifescape import reach as reach_mod
from lifescape.bfs import FAR as BFS_FAR
from lifescape.route import Router


def main():
    loctypes = collision_mod.load_types(os.path.join(ROOT, "lifescape", "data", "gametypes.json"))
    pack = os.path.normpath(os.path.join(ROOT, "..", "lostcity", "engine", "data", "pack"))
    collision, static_locs, _static_actions, _static_layers = collision_mod.load_maps(pack, loctypes)
    freemap = f2p_mod.load_freemap(f2p_mod.csv_path(pack))
    wall_doors = [s for s in doors_mod.find_openables(pack, loctypes) if doors_mod.is_door(loctypes, s[9])]

    opened = collision.copy()
    for spot in wall_doors:
        loc = static_locs.get((spot[0], spot[1], spot[2]))
        if loc is None:
            continue
        entry = loctypes["loc"][loc[0]]
        collision_mod.change_loc_collision(
            opened, loc[1], loc[2], entry["blockrange"], entry["length"], entry["width"],
            entry["active"], spot[0], spot[1], spot[2], False)

    banks = [b for b in knowledge.all_banks() if not b.members and b.level == 0]
    clusters = []
    for bank in sorted(banks, key=lambda e: (e.x, e.z)):
        for cluster in clusters:
            if any(abs(bank.x - x) + abs(bank.z - z) <= 15 for x, z in cluster):
                cluster.append((bank.x, bank.z))
                break
        else:
            clusters.append([(bank.x, bank.z)])

    router = Router()
    stands = []
    for cluster in clusters:
        cx = sum(x for x, _z in cluster) // len(cluster)
        cz = sum(z for _x, z in cluster) // len(cluster)
        booths = []
        for x, z in cluster:
            loc = static_locs.get((x, z, 0))
            if loc is None:
                continue
            booths.append((x, z, loc[0], loc[1], loc[2]))
        # Flood depuis le terrain ouvert autour (pas le comptoir, qui
        # est une poche) : union des floods, stop des qu'on est large.
        opens = set()
        for radius in range(20, 26):
            for dx in range(-radius, radius + 1):
                for dz in (-radius, radius):
                    for t in ((cx + dx, cz + dz), (cx + dz, cz + dx)):
                        if not collision_mod.tile_walkable(opened.get(t[0], t[1], 0)):
                            continue
                        router.find_path(opened, 0, t[0], t[1], t[0] + 100, t[1] + 100, move_near=False)
                        base_x, base_z = t[0] - 64, t[1] - 64
                        for lx in range(128):
                            for lz in range(128):
                                if router.distances[lx * 128 + lz] < BFS_FAR:
                                    opens.add((base_x + lx, base_z + lz))
                        if len(opens) >= 8000:
                            break
                    if len(opens) >= 8000:
                        break
                if len(opens) >= 8000:
                    break
            if len(opens) >= 8000:
                break
        print("bank %s: open %d" % (cluster[0], len(opens)), flush=True)
        if len(opens) < 1500:
            print("bank %s: sealed pocket?" % (cluster[0],), flush=True)
            continue
        best = None
        for (x, z) in opens:
            if abs(x - cx) + abs(z - cz) > 10:
                continue
            if not f2p_mod.is_f2p(freemap, x, z):
                continue
            for bx, bz, loc_id, shape, angle in booths:
                entry = loctypes["loc"][loc_id]
                try:
                    width = int(entry.get("width", 1) or 1)
                    height = int(entry.get("length", 1) or 1)
                except (TypeError, ValueError):
                    width, height = 1, 1
                if reach_mod.reached(opened.get, 0, x, z, bx, bz, width, height, angle, shape, 0):
                    dist = abs(x - cx) + abs(z - cz)
                    if best is None or dist < best[0]:
                        best = (dist, (x, z))
                    break
        if best is None:
            print("bank %s: no stand in reach" % (cluster[0],), flush=True)
            continue
        (_dist, (sx, sz)) = best
        stands.append({"bank": list(cluster[0]), "centroid": [cx, cz],
                       "stand": [sx, sz, 0],
                       "booths": [[x, z] for x, z, _l, _s, _a in booths]})
        print("bank %s -> stand (%d,%d)" % (cluster[0], sx, sz), flush=True)
    with open(OUT, "w") as handle:
        json.dump(stands, handle, indent=1)
    print("wrote %s (%d stands)" % (OUT, len(stands)))


if __name__ == "__main__":
    main()
