"""BFS local, copie du serveur : fenetre 128, anneau 4096.

Le BFS ne connait que des tuiles. La question "suis-je arrive ?" est
deleguee a reach.reached (mur = a cote, objet = autour).
"""

from . import collision as flags
from . import reach as reach_mod

SEARCH = 128
HALF = SEARCH // 2
RING = 1 << 12
FAR = 99999999
MAX_SEEK = 100
APPROACH_RANGE = 10
LOWEST_COST = 1000

NORTH = 0x1
EAST = 0x2
SOUTH = 0x4
WEST = 0x8


def _can_move(tile_flag, block_flag):
    return (tile_flag & block_flag) == flags.OPEN


class Bfs:
    def __init__(self):
        size = SEARCH * SEARCH
        self.directions = [0] * size
        self.distances = [FAR] * size
        self.queue_x = [0] * RING
        self.queue_z = [0] * RING
        self.read = 0
        self.write = 0
        self.cur_x = 0
        self.cur_z = 0

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

    def search(self, collision, base_x, base_z, level,
               local_dest_x, local_dest_z, dest_width, dest_height, angle, shape, block_access):
        limit = SEARCH - 1
        get = collision.get
        while self.write != self.read:
            cur_x = self.queue_x[self.read]
            cur_z = self.queue_z[self.read]
            self.read = (self.read + 1) & (RING - 1)
            if reach_mod.reached(get, level, cur_x + base_x, cur_z + base_z,
                                 local_dest_x + base_x, local_dest_z + base_z,
                                 dest_width, dest_height, angle, shape, block_access):
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
            if (cur_x > 0 and cur_z > 0
                    and self.directions[self._index(cur_x - 1, cur_z - 1)] == 0
                    and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z - 1, level), flags.BLOCK_SOUTH_WEST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z, level), flags.BLOCK_WEST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z - 1, level), flags.BLOCK_SOUTH)):
                self._append(cur_x - 1, cur_z - 1, NORTH | EAST, distance)
            if (cur_x < limit and cur_z > 0
                    and self.directions[self._index(cur_x + 1, cur_z - 1)] == 0
                    and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z - 1, level), flags.BLOCK_SOUTH_EAST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z, level), flags.BLOCK_EAST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z - 1, level), flags.BLOCK_SOUTH)):
                self._append(cur_x + 1, cur_z - 1, NORTH | WEST, distance)
            if (cur_x > 0 and cur_z < limit
                    and self.directions[self._index(cur_x - 1, cur_z + 1)] == 0
                    and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z + 1, level), flags.BLOCK_NORTH_WEST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x - 1, cur_z, level), flags.BLOCK_WEST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z + 1, level), flags.BLOCK_NORTH)):
                self._append(cur_x - 1, cur_z + 1, SOUTH | EAST, distance)
            if (cur_x < limit and cur_z < limit
                    and self.directions[self._index(cur_x + 1, cur_z + 1)] == 0
                    and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z + 1, level), flags.BLOCK_NORTH_EAST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x + 1, cur_z, level), flags.BLOCK_EAST)
                    and _can_move(self._flag(collision, base_x, base_z, cur_x, cur_z + 1, level), flags.BLOCK_NORTH)):
                self._append(cur_x + 1, cur_z + 1, SOUTH | WEST, distance)
        return False

    def closest_approach(self, local_dest_x, local_dest_z, dest_width, dest_height, angle):
        """Point atteignable le plus proche, comme le serveur."""
        width = reach_mod.rotate(angle, dest_width, dest_height)
        height = reach_mod.rotate(angle, dest_height, dest_width)
        lowest = LOWEST_COST
        best_dist = MAX_SEEK
        for x in range(local_dest_x - APPROACH_RANGE, local_dest_x + APPROACH_RANGE + 1):
            for z in range(local_dest_z - APPROACH_RANGE, local_dest_z + APPROACH_RANGE + 1):
                if not 0 <= x < SEARCH or not 0 <= z < SEARCH:
                    continue
                path_dist = self.distances[self._index(x, z)]
                if path_dist >= MAX_SEEK:
                    continue
                dx = 0
                if x < local_dest_x:
                    dx = local_dest_x - x
                elif x > local_dest_x + width - 1:
                    dx = x - (width + local_dest_x - 1)
                dz = 0
                if z < local_dest_z:
                    dz = local_dest_z - z
                elif z > local_dest_z + height - 1:
                    dz = z - (height + local_dest_z - 1)
                cost = dx * dx + dz * dz
                if cost < lowest or (cost == lowest and best_dist > path_dist):
                    self.cur_x, self.cur_z = x, z
                    lowest, best_dist = cost, path_dist
        return lowest != LOWEST_COST

    def compress(self, base_x, base_z, level, src_x, src_z, limit):
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

    def reset(self, local_src_x=HALF, local_src_z=HALF):
        size = SEARCH * SEARCH
        self.directions = [0] * size
        self.distances = [FAR] * size
        self.read = 0
        self.write = 0
        self._append(local_src_x, local_src_z, 99, 0)
