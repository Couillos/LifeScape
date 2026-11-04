"""F2P en dur : le serveur tourne en members=false, le routage le sait.

La carte vient du meme fichier que le serveur
(content/maps/free2play.csv via GameMap.freemap) : un set de zones 8x8.
Tuile F2P = sa zone (niveau force a 0, comme isFreeToPlay) dans le set.

F2PMap est une vue lecture seule : les tuiles members repondent
WALK_BLOCKED, le reste delegue. Les copies de collision (retraits de
portes, dalles) restent mutees en dessous, le routage ne voit que la vue.
"""

import os

from . import collision as collision_mod
from .collision import zone_index


def csv_path(pack_dir):
    """content/maps/free2play.csv depuis le pack dir du serveur."""
    return os.path.normpath(os.path.join(
        os.path.dirname(os.path.dirname(pack_dir)), "..", "content", "maps", "free2play.csv"))


def load_freemap(path):
    """Set de zones F2P (lignes y_mx_mz_lx_lz, cf GameMap.loadCsvMap)."""
    free = set()
    with open(path, encoding="ascii") as handle:
        for line in handle.read().splitlines():
            line = line.strip()
            if not line or line.startswith("//"):
                continue
            level, mx, mz, lx, lz = (int(part) for part in line.split("_"))
            free.add(zone_index((mx << 6) + lx, (mz << 6) + lz, level))
    return free


def is_f2p(freemap, x, z):
    """Comme GameMap.isFreeToPlay : niveau ignore."""
    if freemap is None:
        return True
    return zone_index(x, z, 0) in freemap


class F2PMap:
    """Vue routage : members bloque, F2P delegue. Lecture seule."""

    def __init__(self, base, freemap):
        self.base = base
        self.freemap = freemap

    def get(self, x, z, level):
        if not is_f2p(self.freemap, x, z):
            return collision_mod.WALK_BLOCKED
        return self.base.get(x, z, level)

    def has(self, x, z, level):
        return self.base.has(x, z, level) if hasattr(self.base, "has") else True
