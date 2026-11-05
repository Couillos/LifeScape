//! Router: local 128 BFS + server ReachStrategy.
//!
//! Exact port of lifescape/route.py. Wall/object targets stop on the
//! interaction tile, not on top of it. Long distances need portals.

use crate::bfs::{self, Bfs, CollisionSource};
use crate::collision;

/// One direction-change point on the way back from the target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Waypoint {
    pub level: i32,
    pub x: i32,
    pub z: i32,
}

pub struct Router {
    pub bfs: Bfs,
}

impl Router {
    pub fn new() -> Self {
        Self { bfs: Bfs::new() }
    }

    pub fn distances(&self) -> &[i32] {
        &self.bfs.distances
    }

    pub fn directions(&self) -> &[u8] {
        &self.bfs.directions
    }

    pub fn find_path(
        &mut self,
        collision: &impl CollisionSource,
        level: i32,
        src_x: i32,
        src_z: i32,
        dest_x: i32,
        dest_z: i32,
        max_waypoints: usize,
        move_near: bool,
        dest_width: i32,
        dest_height: i32,
        angle: i32,
        shape: i32,
        block_access: u32,
    ) -> Vec<Waypoint> {
        self.bfs.reset();
        let base_x = src_x - bfs::HALF;
        let base_z = src_z - bfs::HALF;
        let local_dest_x = dest_x - base_x;
        let local_dest_z = dest_z - base_z;
        let found = self.bfs.search(
            collision,
            base_x,
            base_z,
            level,
            local_dest_x,
            local_dest_z,
            dest_width,
            dest_height,
            angle,
            shape,
            block_access,
        );
        if !found {
            if !move_near {
                return Vec::new();
            }
            if !self
                .bfs
                .closest_approach(local_dest_x, local_dest_z, dest_width, dest_height, angle)
            {
                return Vec::new();
            }
        }
        self.bfs
            .compress(base_x, base_z, level, bfs::HALF, bfs::HALF, max_waypoints)
    }

    /// Plain tile target with the default budget (25 waypoints, move near).
    pub fn find_path_to_tile(
        &mut self,
        collision: &impl CollisionSource,
        level: i32,
        src_x: i32,
        src_z: i32,
        dest_x: i32,
        dest_z: i32,
    ) -> Vec<Waypoint> {
        self.find_path(
            collision,
            level,
            src_x,
            src_z,
            dest_x,
            dest_z,
            25,
            true,
            1,
            1,
            0,
            crate::reach::SHAPE_TILE,
            0,
        )
    }

    /// Why it cannot get there: closest reachable tile plus blockers.
    pub fn unreachable_report(
        &mut self,
        collision: &impl CollisionSource,
        level: i32,
        src_x: i32,
        src_z: i32,
        dest_x: i32,
        dest_z: i32,
        name_at: Option<&dyn Fn(i32, i32, i32) -> Option<String>>,
        limit: usize,
        dest_width: i32,
        dest_height: i32,
        angle: i32,
        shape: i32,
        block_access: u32,
    ) -> UnreachableReport {
        self.find_path(
            collision,
            level,
            src_x,
            src_z,
            dest_x,
            dest_z,
            25,
            false,
            dest_width,
            dest_height,
            angle,
            shape,
            block_access,
        );
        let base_x = src_x - bfs::HALF;
        let base_z = src_z - bfs::HALF;
        let mut nearest: Option<((i32, i32), i32, i32, i32)> = None;
        for tile_x in (dest_x - bfs::APPROACH_RANGE)..=(dest_x + bfs::APPROACH_RANGE) {
            for tile_z in (dest_z - bfs::APPROACH_RANGE)..=(dest_z + bfs::APPROACH_RANGE) {
                let local_x = tile_x - base_x;
                let local_z = tile_z - base_z;
                if !(0 <= local_x && local_x < bfs::SEARCH && 0 <= local_z && local_z < bfs::SEARCH) {
                    continue;
                }
                let path_distance =
                    self.distances()[(local_x * bfs::SEARCH + local_z) as usize];
                if path_distance >= bfs::MAX_SEEK {
                    continue;
                }
                let spread = (tile_x - dest_x).abs() + (tile_z - dest_z).abs();
                let rank = (spread, path_distance);
                let replace = match nearest {
                    None => true,
                    Some((best_rank, _, _, _)) => rank < best_rank,
                };
                if replace {
                    nearest = Some((rank, tile_x, tile_z, path_distance));
                }
            }
        }
        let mut blockers: Vec<(i32, i32, String)> = Vec::new();
        for local_x in 0..bfs::SEARCH {
            for local_z in 0..bfs::SEARCH {
                if self.directions()[(local_x * bfs::SEARCH + local_z) as usize] == 0 {
                    continue;
                }
                for (step_x, step_z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let near_x = local_x + step_x;
                    let near_z = local_z + step_z;
                    if !(0 <= near_x && near_x < bfs::SEARCH && 0 <= near_z && near_z < bfs::SEARCH)
                    {
                        continue;
                    }
                    if self.directions()[(near_x * bfs::SEARCH + near_z) as usize] != 0 {
                        continue;
                    }
                    let block_x = base_x + near_x;
                    let block_z = base_z + near_z;
                    if collision.get(block_x, block_z, level) == collision::OPEN {
                        continue;
                    }
                    let name = name_at.and_then(|lookup| lookup(block_x, block_z, level));
                    if let Some(name) = name {
                        if !blockers
                            .iter()
                            .any(|existing| existing.0 == block_x && existing.1 == block_z && existing.2 == name)
                        {
                            blockers.push((block_x, block_z, name));
                        }
                    }
                }
            }
        }
        blockers.sort_by_key(|blocker| (blocker.0 - dest_x).abs() + (blocker.1 - dest_z).abs());
        blockers.truncate(limit);
        UnreachableReport {
            nearest: nearest.map(|(_, tile_x, tile_z, _)| (tile_x, tile_z)),
            path_distance: nearest.map(|(_, _, _, dist)| dist),
            spread: nearest.map(|(rank, _, _, _)| rank.0),
            blockers,
            dest_blocked: !collision::tile_walkable(collision.get(dest_x, dest_z, level)),
            dest_name: name_at.and_then(|lookup| lookup(dest_x, dest_z, level)),
        }
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct UnreachableReport {
    pub nearest: Option<(i32, i32)>,
    pub path_distance: Option<i32>,
    pub spread: Option<i32>,
    pub blockers: Vec<(i32, i32, String)>,
    pub dest_blocked: bool,
    pub dest_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionMap;

    struct OpenField;

    impl CollisionSource for OpenField {
        fn get(&self, _x: i32, _z: i32, _level: i32) -> u32 {
            collision::OPEN
        }
    }

    #[test]
    fn open_map_reaches_target() {
        let mut router = Router::new();
        let path = router.find_path_to_tile(&OpenField, 0, 1000, 1000, 1010, 1005);
        assert!(!path.is_empty(), "open map must yield waypoints");
        let last = path.last().unwrap();
        assert_eq!((last.level, last.x, last.z), (0, 1010, 1005));
    }

    #[test]
    fn walled_target_reports_blockers() {
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
        let mut router = Router::new();
        let report = router.unreachable_report(
            &map,
            0,
            1000,
            1000,
            dest_x,
            dest_z,
            Some(&|x, z, _level| Some(format!("wall {x},{z}"))),
            8,
            1,
            1,
            0,
            crate::reach::SHAPE_TILE,
            0,
        );
        assert!(report.dest_blocked);
        assert_eq!(report.nearest, Some((1003, 1003)));
        assert!(!report.blockers.is_empty());
        assert!(report.blockers.len() <= 8);
    }
}
