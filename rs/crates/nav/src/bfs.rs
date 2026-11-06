//! Local BFS, server copy: 128 window, 4096 ring.
//!
//! Local search over the collision flags. The BFS only knows tiles; "have we
//! arrived?" is delegated to reach::reached (wall = next to it,
//! object = around it).

use crate::collision::{self, CollisionMap};
use crate::reach;
use crate::route::Waypoint;

pub const SEARCH: i32 = 128;
pub const HALF: i32 = SEARCH / 2;
pub const RING: usize = 1 << 12;
pub const FAR: i32 = 99999999;
pub const MAX_SEEK: i32 = 100;
pub const APPROACH_RANGE: i32 = 10;
pub const LOWEST_COST: i32 = 1000;

/// Anything the BFS/router can read flags from (real map, door overlay...).
pub trait CollisionSource {
    fn get(&self, x: i32, z: i32, level: i32) -> u32;
}

impl CollisionSource for CollisionMap {
    fn get(&self, x: i32, z: i32, level: i32) -> u32 {
        CollisionMap::get(self, x, z, level)
    }
}

pub const NORTH: u8 = 0x1;
pub const EAST: u8 = 0x2;
pub const SOUTH: u8 = 0x4;
pub const WEST: u8 = 0x8;

fn can_move(tile_flag: u32, block_flag: u32) -> bool {
    (tile_flag & block_flag) == collision::OPEN
}

fn flag_at(
    collision: &impl CollisionSource,
    base_x: i32,
    base_z: i32,
    local_x: i32,
    local_z: i32,
    level: i32,
) -> u32 {
    collision.get(base_x + local_x, base_z + local_z, level)
}

#[derive(Clone, Debug)]
pub struct Bfs {
    pub directions: Vec<u8>,
    pub distances: Vec<i32>,
    queue_x: Vec<i32>,
    queue_z: Vec<i32>,
    read: usize,
    write: usize,
    pub cur_x: i32,
    pub cur_z: i32,
}

impl Bfs {
    pub fn new() -> Self {
        Self {
            directions: vec![0; (SEARCH * SEARCH) as usize],
            distances: vec![FAR; (SEARCH * SEARCH) as usize],
            queue_x: vec![0; RING],
            queue_z: vec![0; RING],
            read: 0,
            write: 0,
            cur_x: 0,
            cur_z: 0,
        }
    }

    fn index(local_x: i32, local_z: i32) -> usize {
        (local_x * SEARCH + local_z) as usize
    }

    fn append(&mut self, x: i32, z: i32, direction: u8, distance: i32) {
        // Live leg tracing: BFS_TRACE=1 prints every queued tile.
        // Zero cost when unset; used with the `map route` CLI command.
        if std::env::var("BFS_TRACE").is_ok() { eprintln!("bfs append ({x},{z}) dist={distance}"); }
        self.directions[Self::index(x, z)] = direction;
        self.distances[Self::index(x, z)] = distance;
        self.queue_x[self.write] = x;
        self.queue_z[self.write] = z;
        self.write = (self.write + 1) & (RING - 1);
    }

    pub fn search(
        &mut self,
        collision: &impl CollisionSource,
        base_x: i32,
        base_z: i32,
        level: i32,
        local_dest_x: i32,
        local_dest_z: i32,
        dest_width: i32,
        dest_height: i32,
        angle: i32,
        shape: i32,
        block_access: u32,
    ) -> bool {
        let limit = SEARCH - 1;
        while self.write != self.read {
            let cur_x = self.queue_x[self.read];
            let cur_z = self.queue_z[self.read];
            self.read = (self.read + 1) & (RING - 1);
            let get_flag = |x: i32, z: i32, flag_level: i32| collision.get(x, z, flag_level);
            if reach::reached(
                &get_flag,
                level,
                cur_x + base_x,
                cur_z + base_z,
                local_dest_x + base_x,
                local_dest_z + base_z,
                dest_width,
                dest_height,
                angle,
                shape,
                block_access,
            ) {
                self.cur_x = cur_x;
                self.cur_z = cur_z;
                return true;
            }
            let distance = self.distances[Self::index(cur_x, cur_z)] + 1;
            if cur_x > 0 && self.directions[Self::index(cur_x - 1, cur_z)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x - 1, cur_z, level),
                    collision::BLOCK_WEST,
                )
            {
                self.append(cur_x - 1, cur_z, EAST, distance);
            }
            if cur_x < limit && self.directions[Self::index(cur_x + 1, cur_z)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x + 1, cur_z, level),
                    collision::BLOCK_EAST,
                )
            {
                self.append(cur_x + 1, cur_z, WEST, distance);
            }
            if cur_z > 0 && self.directions[Self::index(cur_x, cur_z - 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z - 1, level),
                    collision::BLOCK_SOUTH,
                )
            {
                self.append(cur_x, cur_z - 1, NORTH, distance);
            }
            if cur_z < limit && self.directions[Self::index(cur_x, cur_z + 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z + 1, level),
                    collision::BLOCK_NORTH,
                )
            {
                self.append(cur_x, cur_z + 1, SOUTH, distance);
            }
            if cur_x > 0
                && cur_z > 0
                && self.directions[Self::index(cur_x - 1, cur_z - 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x - 1, cur_z - 1, level),
                    collision::BLOCK_SOUTH_WEST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x - 1, cur_z, level),
                    collision::BLOCK_WEST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z - 1, level),
                    collision::BLOCK_SOUTH,
                )
            {
                self.append(cur_x - 1, cur_z - 1, NORTH | EAST, distance);
            }
            if cur_x < limit
                && cur_z > 0
                && self.directions[Self::index(cur_x + 1, cur_z - 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x + 1, cur_z - 1, level),
                    collision::BLOCK_SOUTH_EAST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x + 1, cur_z, level),
                    collision::BLOCK_EAST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z - 1, level),
                    collision::BLOCK_SOUTH,
                )
            {
                self.append(cur_x + 1, cur_z - 1, NORTH | WEST, distance);
            }
            if cur_x > 0
                && cur_z < limit
                && self.directions[Self::index(cur_x - 1, cur_z + 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x - 1, cur_z + 1, level),
                    collision::BLOCK_NORTH_WEST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x - 1, cur_z, level),
                    collision::BLOCK_WEST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z + 1, level),
                    collision::BLOCK_NORTH,
                )
            {
                self.append(cur_x - 1, cur_z + 1, SOUTH | EAST, distance);
            }
            if cur_x < limit
                && cur_z < limit
                && self.directions[Self::index(cur_x + 1, cur_z + 1)] == 0
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x + 1, cur_z + 1, level),
                    collision::BLOCK_NORTH_EAST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x + 1, cur_z, level),
                    collision::BLOCK_EAST,
                )
                && can_move(
                    flag_at(collision, base_x, base_z, cur_x, cur_z + 1, level),
                    collision::BLOCK_NORTH,
                )
            {
                self.append(cur_x + 1, cur_z + 1, SOUTH | WEST, distance);
            }
        }
        false
    }

    /// Closest reachable point, like the server.
    pub fn closest_approach(
        &mut self,
        local_dest_x: i32,
        local_dest_z: i32,
        dest_width: i32,
        dest_height: i32,
        angle: i32,
    ) -> bool {
        let width = reach::rotate(angle, dest_width, dest_height);
        let height = reach::rotate(angle, dest_height, dest_width);
        let mut lowest = LOWEST_COST;
        let mut best_dist = MAX_SEEK;
        for probe_x in (local_dest_x - APPROACH_RANGE)..=(local_dest_x + APPROACH_RANGE) {
            for probe_z in (local_dest_z - APPROACH_RANGE)..=(local_dest_z + APPROACH_RANGE) {
                if !(0 <= probe_x && probe_x < SEARCH && 0 <= probe_z && probe_z < SEARCH) {
                    continue;
                }
                let path_dist = self.distances[Self::index(probe_x, probe_z)];
                if path_dist >= MAX_SEEK {
                    continue;
                }
                let mut step_x = 0;
                if probe_x < local_dest_x {
                    step_x = local_dest_x - probe_x;
                } else if probe_x > local_dest_x + width - 1 {
                    step_x = probe_x - (width + local_dest_x - 1);
                }
                let mut step_z = 0;
                if probe_z < local_dest_z {
                    step_z = local_dest_z - probe_z;
                } else if probe_z > local_dest_z + height - 1 {
                    step_z = probe_z - (height + local_dest_z - 1);
                }
                let cost = step_x * step_x + step_z * step_z;
                if cost < lowest || (cost == lowest && best_dist > path_dist) {
                    self.cur_x = probe_x;
                    self.cur_z = probe_z;
                    lowest = cost;
                    best_dist = path_dist;
                }
            }
        }
        lowest != LOWEST_COST
    }

    pub fn compress(
        &mut self,
        base_x: i32,
        base_z: i32,
        level: i32,
        src_x: i32,
        src_z: i32,
        limit: usize,
    ) -> Vec<Waypoint> {
        let mut waypoints = Vec::new();
        let mut direction = self.directions[Self::index(self.cur_x, self.cur_z)];
        let mut current = 0xFF;
        for _ in 0..(SEARCH * SEARCH) {
            if self.cur_x == src_x && self.cur_z == src_z {
                break;
            }
            if current != direction {
                current = direction;
                if waypoints.len() >= limit {
                    waypoints.pop();
                }
                waypoints.insert(
                    0,
                    Waypoint {
                        level,
                        x: base_x + self.cur_x,
                        z: base_z + self.cur_z,
                    },
                );
            }
            if current & EAST != 0 {
                self.cur_x += 1;
            } else if current & WEST != 0 {
                self.cur_x -= 1;
            }
            if current & NORTH != 0 {
                self.cur_z += 1;
            } else if current & SOUTH != 0 {
                self.cur_z -= 1;
            }
            direction = self.directions[Self::index(self.cur_x, self.cur_z)];
        }
        waypoints
    }

    pub fn reset(&mut self) {
        self.reset_at(HALF, HALF);
    }

    pub fn reset_at(&mut self, local_src_x: i32, local_src_z: i32) {
        self.directions = vec![0; (SEARCH * SEARCH) as usize];
        self.distances = vec![FAR; (SEARCH * SEARCH) as usize];
        self.read = 0;
        self.write = 0;
        self.append(local_src_x, local_src_z, 99, 0);
    }
}

impl Default for Bfs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OpenField;

    impl CollisionSource for OpenField {
        fn get(&self, _x: i32, _z: i32, _level: i32) -> u32 {
            collision::OPEN
        }
    }

    fn run_search(field: &impl CollisionSource, dest_x: i32, dest_z: i32) -> Bfs {
        let mut bfs = Bfs::new();
        bfs.reset();
        let base_x = 1000 - HALF;
        let base_z = 1000 - HALF;
        let found = bfs.search(
            field,
            base_x,
            base_z,
            0,
            dest_x - base_x,
            dest_z - base_z,
            1,
            1,
            0,
            reach::SHAPE_TILE,
            0,
        );
        assert!(found, "target must be reached on an open map");
        bfs
    }

    #[test]
    fn reaches_target_on_empty_map() {
        let field = OpenField;
        let mut bfs = run_search(&field, 1010, 1005);
        let waypoints = bfs.compress(1000 - HALF, 1000 - HALF, 0, HALF, HALF, 25);
        assert_eq!(waypoints.len(), 2, "known-good compression on this scenario");
        let last = waypoints.last().unwrap();
        assert_eq!((last.x, last.z), (1010, 1005));
        assert!(waypoints.len() <= 25);
    }

    #[test]
    fn closest_approach_on_walled_target() {
        let dest_x = 1005;
        let dest_z = 1003;
        let mut map = CollisionMap::new();
        for tile_x in (dest_x - 20)..=(dest_x + 20) {
            for tile_z in (dest_z - 20)..=(dest_z + 20) {
                map.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        for ring_x in -1..=1 {
            for ring_z in -1..=1 {
                map.set(dest_x + ring_x, dest_z + ring_z, 0, collision::WALK_BLOCKED);
            }
        }
        let mut bfs = Bfs::new();
        bfs.reset();
        let base_x = 1000 - HALF;
        let base_z = 1000 - HALF;
        let found = bfs.search(
            &map,
            base_x,
            base_z,
            0,
            dest_x - base_x,
            dest_z - base_z,
            1,
            1,
            0,
            reach::SHAPE_TILE,
            0,
        );
        assert!(!found, "walled target must not be reached");
        assert!(
            bfs.closest_approach(dest_x - base_x, dest_z - base_z, 1, 1, 0),
            "closest approach must find a reachable tile"
        );
        let reached_x = base_x + bfs.cur_x;
        let reached_z = base_z + bfs.cur_z;
        assert_eq!(
            (reached_x, reached_z),
            (1003, 1003),
            "known-good compression on this scenario"
        );
        assert!(
            collision::tile_walkable(map.get(reached_x, reached_z, 0)),
            "approach point must be walkable"
        );
    }
}
