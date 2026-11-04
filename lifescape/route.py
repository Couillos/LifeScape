"""Routeur : BFS local 128 + ReachStrategy serveur.

Formes cibles (murs, objets) : s'arrete a la tuile d'interaction,
pas dessus. Distances longues : voir portals.RoutePlanner.
"""

from . import collision as flags
from .bfs import Bfs, SEARCH, HALF, FAR, MAX_SEEK, APPROACH_RANGE


class Router:
    def __init__(self):
        self.bfs = Bfs()

    @property
    def distances(self):
        return self.bfs.distances

    @property
    def directions(self):
        return self.bfs.directions

    def find_path(self, collision, level, src_x, src_z, dest_x, dest_z,
                  max_waypoints=25, move_near=True,
                  dest_width=1, dest_height=1, angle=0, shape=-1, block_access=0):
        self.bfs.reset()
        base_x = src_x - HALF
        base_z = src_z - HALF
        local_dest_x = dest_x - base_x
        local_dest_z = dest_z - base_z
        found = self.bfs.search(collision, base_x, base_z, level, local_dest_x, local_dest_z,
                                dest_width, dest_height, angle, shape, block_access)
        if not found:
            if not move_near:
                return []
            if not self.bfs.closest_approach(local_dest_x, local_dest_z, dest_width, dest_height, angle):
                return []
        return self.bfs.compress(base_x, base_z, level, HALF, HALF, max_waypoints)

    def unreachable_report(self, collision, level, src_x, src_z, dest_x, dest_z, name_at=None, limit=8,
                           dest_width=1, dest_height=1, angle=0, shape=-1, block_access=0):
        """Pourquoi ca ne passe pas : plus proche atteignable + blocages."""
        self.find_path(collision, level, src_x, src_z, dest_x, dest_z, move_near=False,
                       dest_width=dest_width, dest_height=dest_height,
                       angle=angle, shape=shape, block_access=block_access)
        base_x = src_x - HALF
        base_z = src_z - HALF
        nearest = None
        blockers = []
        for tile_x in range(dest_x - APPROACH_RANGE, dest_x + APPROACH_RANGE + 1):
            for tile_z in range(dest_z - APPROACH_RANGE, dest_z + APPROACH_RANGE + 1):
                local_x = tile_x - base_x
                local_z = tile_z - base_z
                if not (0 <= local_x < SEARCH and 0 <= local_z < SEARCH):
                    continue
                path_distance = self.distances[local_x * SEARCH + local_z]
                if path_distance >= MAX_SEEK:
                    continue
                spread = abs(tile_x - dest_x) + abs(tile_z - dest_z)
                rank = (spread, path_distance)
                if nearest is None or rank < nearest[0]:
                    nearest = (rank, tile_x, tile_z, path_distance)
        for local_x in range(SEARCH):
            row = local_x * SEARCH
            for local_z in range(SEARCH):
                if self.directions[row + local_z] == 0:
                    continue
                for step_x, step_z in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    near_x = local_x + step_x
                    near_z = local_z + step_z
                    if not (0 <= near_x < SEARCH and 0 <= near_z < SEARCH):
                        continue
                    if self.directions[near_x * SEARCH + near_z] != 0:
                        continue
                    block_x = base_x + near_x
                    block_z = base_z + near_z
                    if collision.get(block_x, block_z, level) == flags.OPEN:
                        continue
                    name = name_at(block_x, block_z, level) if name_at else None
                    if name and (block_x, block_z, name) not in blockers:
                        blockers.append((block_x, block_z, name))
        blockers.sort(key=lambda item: abs(item[0] - dest_x) + abs(item[1] - dest_z))
        return {
            "nearest": (nearest[1], nearest[2]) if nearest else None,
            "path_distance": nearest[3] if nearest else None,
            "spread": nearest[0][0] if nearest else None,
            "blockers": blockers[:limit],
            "dest_blocked": not flags.tile_walkable(collision.get(dest_x, dest_z, level)),
            "dest_name": name_at(dest_x, dest_z, level) if name_at else None,
        }
