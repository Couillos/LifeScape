"""Breadth-first routing, mirroring the server PathFinder for size-1 movers.

Same grid, same diagonal rules, same waypoint compression, so a path
the SDK computes is a path the server walks.
"""

from . import collision as flags

# 256x256 tiles (128 each way): a fence or a building can push the
# real route that far out, and a smaller window reports "no path".
SEARCH = 256
HALF = SEARCH // 2
RING = 1 << 17
FAR = 99999999
# No effective cap: the BFS already covers the whole window, and F2P walks
# (castle to the Wizards' Tower around the swamp) legitimately run several
# hundred tiles. Capping here silently turned long-but-real routes into
# "no path". Unreached tiles keep distance FAR and are still excluded.
MAX_SEEK = 100000
APPROACH_RANGE = 10
LOWEST_COST = 1000

NORTH = 0x1
EAST = 0x2
SOUTH = 0x4
WEST = 0x8


def expand_steps(waypoints, start, cap=24):
    steps = []
    here = start
    for _, wx, wz in waypoints:
        while here != (wx, wz):
            dx = (wx > here[0]) - (wx < here[0])
            dz = (wz > here[1]) - (wz < here[1])
            here = (here[0] + dx, here[1] + dz)
            steps.append(here)
            if len(steps) >= cap:
                return steps
    return steps


def _can_move(tile_flag, block_flag):
    return (tile_flag & block_flag) == flags.OPEN


class Router:
    def __init__(self):
        size = SEARCH * SEARCH
        self.directions = [0] * size
        self.distances = [FAR] * size
        self.queue_x = [0] * RING
        self.queue_z = [0] * RING
        self.read = 0
        self.write = 0

    def find_path(self, collision, level, src_x, src_z, dest_x, dest_z, max_waypoints=25, move_near=True):
        self.directions = [0] * (SEARCH * SEARCH)
        self.distances = [FAR] * (SEARCH * SEARCH)
        self.read = 0
        self.write = 0

        base_x = src_x - HALF
        base_z = src_z - HALF
        local_dest_x = dest_x - base_x
        local_dest_z = dest_z - base_z

        self._append(HALF, HALF, 99, 0)
        found = self._search(collision, base_x, base_z, level, local_dest_x, local_dest_z)

        if not found:
            if not move_near:
                return []
            dest_blocked = not flags.tile_walkable(collision.get(dest_x, dest_z, level))
            if not self._closest_approach(local_dest_x, local_dest_z, prefer_path=dest_blocked):
                return []

        return self._compress(base_x, base_z, level, HALF, HALF, max_waypoints)

    def _index(self, x, z):
        return x * SEARCH + z

    def _flag(self, collision, base_x, base_z, local_x, local_z, level):
        return collision.get(base_x + local_x, base_z + local_z, level)

    def _append(self, x, z, direction, distance):
        self.directions[self._index(x, z)] = direction
        self.distances[self._index(x, z)] = distance
        self.queue_x[self.write] = x
        self.queue_z[self.write] = z
        self.write = (self.write + 1) & (RING - 1)

    def _search(self, collision, base_x, base_z, level, dest_x, dest_z):
        limit = SEARCH - 1
        while self.write != self.read:
            cur_x = self.queue_x[self.read]
            cur_z = self.queue_z[self.read]
            self.read = (self.read + 1) & (RING - 1)

            if cur_x == dest_x and cur_z == dest_z:
                self.cur_x, self.cur_z = cur_x, cur_z
                return True

            distance = self.distances[self._index(cur_x, cur_z)] + 1

            if cur_x > 0 and self.directions[self._index(cur_x - 1, cur_z)] == 0:
                if _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z, level), flags.BLOCK_WEST):
                    self._append(cur_x - 1, cur_z, EAST, distance)

            if cur_x < limit and self.directions[self._index(cur_x + 1, cur_z)] == 0:
                if _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z, level), flags.BLOCK_EAST):
                    self._append(cur_x + 1, cur_z, WEST, distance)

            if cur_z > 0 and self.directions[self._index(cur_x, cur_z - 1)] == 0:
                if _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z - 1, level), flags.BLOCK_SOUTH):
                    self._append(cur_x, cur_z - 1, NORTH, distance)

            if cur_z < limit and self.directions[self._index(cur_x, cur_z + 1)] == 0:
                if _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z + 1, level), flags.BLOCK_NORTH):
                    self._append(cur_x, cur_z + 1, SOUTH, distance)

            if (
                cur_x > 0
                and cur_z > 0
                and self.directions[self._index(cur_x - 1, cur_z - 1)] == 0
                and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z - 1, level), flags.BLOCK_SOUTH_WEST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z, level), flags.BLOCK_WEST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z - 1, level), flags.BLOCK_SOUTH)
            ):
                self._append(cur_x - 1, cur_z - 1, NORTH | EAST, distance)

            if (
                cur_x < limit
                and cur_z > 0
                and self.directions[self._index(cur_x + 1, cur_z - 1)] == 0
                and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z - 1, level), flags.BLOCK_SOUTH_EAST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z, level), flags.BLOCK_EAST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z - 1, level), flags.BLOCK_SOUTH)
            ):
                self._append(cur_x + 1, cur_z - 1, NORTH | WEST, distance)

            if (
                cur_x > 0
                and cur_z < limit
                and self.directions[self._index(cur_x - 1, cur_z + 1)] == 0
                and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z + 1, level), flags.BLOCK_NORTH_WEST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z, level), flags.BLOCK_WEST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z + 1, level), flags.BLOCK_NORTH)
            ):
                self._append(cur_x - 1, cur_z + 1, SOUTH | EAST, distance)

            if (
                cur_x < limit
                and cur_z < limit
                and self.directions[self._index(cur_x + 1, cur_z + 1)] == 0
                and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z + 1, level), flags.BLOCK_NORTH_EAST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z, level), flags.BLOCK_EAST)
                and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z + 1, level), flags.BLOCK_NORTH)
            ):
                self._append(cur_x + 1, cur_z + 1, SOUTH | WEST, distance)

        return False

    def _closest_approach(self, dest_x, dest_z, prefer_path=False):
        """Pick the reachable tile nearest the destination.

        The closest ring around the target wins (a wall, table or booth
        keeps the interaction range), then the shortest walk inside that
        ring. ``prefer_path`` is kept for callers that want the same rule
        spelled out; the ranking is already path-aware.
        """
        best = None
        for x in range(dest_x - APPROACH_RANGE, dest_x + APPROACH_RANGE + 1):
            for z in range(dest_z - APPROACH_RANGE, dest_z + APPROACH_RANGE + 1):
                if not 0 <= x < SEARCH or not 0 <= z < SEARCH:
                    continue
                path_distance = self.distances[self._index(x, z)]
                if path_distance >= MAX_SEEK:
                    continue
                cost = abs(x - dest_x) * abs(x - dest_x) + abs(z - dest_z) * abs(z - dest_z)
                rank = (cost, path_distance)
                if best is None or rank < best[0]:
                    best = (rank, x, z)
        if best is None:
            return False
        self.cur_x, self.cur_z = best[1], best[2]
        return True

    def unreachable_report(self, collision, level, src_x, src_z, dest_x, dest_z, name_at=None, limit=8):
        """Why the last search could not reach (dest_x, dest_z).

        Runs the same bounded BFS as find_path, then reports the closest
        reachable tile and the named locs blocking the frontier. ``name_at``
        is an optional (x, z, level) -> name callback so this module stays
        independent from gamedata.
        """
        self.find_path(collision, level, src_x, src_z, dest_x, dest_z, move_near=False)
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
                tile_x = base_x + local_x
                tile_z = base_z + local_z
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

    def _compress(self, base_x, base_z, level, src_x, src_z, limit):
        waypoints = []
        direction = self.directions[self._index(self.cur_x, self.cur_z)]
        current = -1
        for _ in range(SEARCH * SEARCH):
            if self.cur_x == src_x and self.cur_z == src_z:
                break
            if current != direction:
                current = direction
                if len(waypoints) >= limit:
                    waypoints.pop()
                waypoints.insert(0, (level, base_x + self.cur_x, base_z + self.cur_z))
            if current & EAST:
                self.cur_x += 1
            elif current & WEST:
                self.cur_x -= 1
            if current & NORTH:
                self.cur_z += 1
            elif current & SOUTH:
                self.cur_z -= 1
            direction = self.directions[self._index(self.cur_x, self.cur_z)]
        return waypoints
