//! Door-aware navigation planning over the cached collision map.
//!
//! The planner
//! treats every openable door footprint as walkable while planning, then
//! annotates the first tile of each door crossed. The walker walks up to a
//! door, opens it, and continues with the same plan.
//!
//! shortcut: the F2P freemap check is an optional `outside` hook on DoorMap
//! instead of a ported map file; pass None when routing without F2P bounds.

use std::collections::{HashMap, HashSet};

use crate::bfs::CollisionSource;
use crate::collision::{self, CollisionMap};
use crate::route::{Router, Waypoint};

/// All single steps from start through the waypoints, no cap.
pub fn expand_waypoints(waypoints: &[Waypoint], start_x: i32, start_z: i32) -> Vec<(i32, i32)> {
    let mut steps = Vec::new();
    let mut here = (start_x, start_z);
    for waypoint in waypoints {
        while here != (waypoint.x, waypoint.z) {
            let step_x = (waypoint.x > here.0) as i32 - (waypoint.x < here.0) as i32;
            let step_z = (waypoint.z > here.1) as i32 - (waypoint.z < here.1) as i32;
            here = (here.0 + step_x, here.1 + step_z);
            steps.push(here);
        }
    }
    steps
}

/// Collision view where a set of door tiles reads open for planning.
pub struct DoorMap<'a, C: CollisionSource> {
    base: &'a C,
    doors: &'a HashSet<(i32, i32, i32)>,
    /// Returns true for tiles outside the allowed area (F2P bounds).
    outside: Option<&'a dyn Fn(i32, i32) -> bool>,
}

impl<'a, C: CollisionSource> DoorMap<'a, C> {
    pub fn new(
        base: &'a C,
        doors: &'a HashSet<(i32, i32, i32)>,
        outside: Option<&'a dyn Fn(i32, i32) -> bool>,
    ) -> Self {
        Self {
            base,
            doors,
            outside,
        }
    }
}

impl<C: CollisionSource> CollisionSource for DoorMap<'_, C> {
    fn get(&self, x: i32, z: i32, level: i32) -> u32 {
        if let Some(outside) = self.outside {
            if outside(x, z) {
                return collision::WALK_BLOCKED;
            }
        }
        if self.doors.contains(&(x, z, level)) {
            return collision::OPEN;
        }
        self.base.get(x, z, level)
    }
}

/// Static door footprint: the wall-door spot tuple
/// (x, z, level, shape, angle, blockrange, length, width, active, loc).
#[derive(Clone, Debug)]
pub struct DoorSpot {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub shape: u8,
    pub angle: u8,
    pub blockrange: bool,
    pub length: i32,
    pub width: i32,
    pub active: i32,
    pub loc_id: usize,
}

/// Gamedata needed per door loc: display name, category, resolved open op.
#[derive(Clone, Debug)]
pub struct LocMeta {
    pub name: String,
    pub category: i32,
    pub open_op: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct DoorAction {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub loc_id: usize,
    pub name: String,
    pub op: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub steps: Vec<(i32, i32)>,
    pub actions: Vec<(usize, DoorAction)>,
    pub target: (i32, i32),
}

pub struct Planner {
    doors: Vec<DoorSpot>,
    locs: HashMap<usize, LocMeta>,
    door_tiles: HashMap<(i32, i32, i32), usize>,
    single_tiles: HashSet<(i32, i32, i32)>,
    footprints: Vec<HashSet<(i32, i32, i32)>>,
}

impl Planner {
    pub fn new(doors: Vec<DoorSpot>, locs: HashMap<usize, LocMeta>) -> Self {
        let mut door_tiles = HashMap::new();
        let mut single_tiles = HashSet::new();
        let mut footprints = Vec::with_capacity(doors.len());
        for (index, spot) in doors.iter().enumerate() {
            let tiles: HashSet<(i32, i32, i32)> = footprint(spot).into_iter().collect();
            footprints.push(tiles.clone());
            let category = locs
                .get(&spot.loc_id)
                .map(|meta| meta.category)
                .unwrap_or(-1);
            for tile in &tiles {
                door_tiles.insert(*tile, index);
                if category != 21 && category != 23 {
                    single_tiles.insert(*tile);
                }
            }
        }
        Self {
            doors,
            locs,
            door_tiles,
            single_tiles,
            footprints,
        }
    }

    /// Index of the closed door standing on this tile, if any.
    pub fn closed_door_at(
        &self,
        x: i32,
        z: i32,
        level: i32,
        is_closed: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        let index = *self.door_tiles.get(&(x, z, level))?;
        if is_closed(index) {
            Some(index)
        } else {
            None
        }
    }

    pub fn plan(
        &self,
        router: &mut Router,
        base: &impl CollisionSource,
        level: i32,
        start_x: i32,
        start_z: i32,
        dest_x: i32,
        dest_z: i32,
        is_closed: &dyn Fn(usize) -> bool,
        blocked_doors: &[usize],
    ) -> Option<Plan> {
        let blocked: HashSet<(i32, i32, i32)> = blocked_doors
            .iter()
            .flat_map(|door| self.footprints[*door].iter().copied())
            .collect();
        // Prefer ordinary doors: double doors swing onto adjacent tiles and
        // their open halves can block the doorway on the client map.
        let single: HashSet<(i32, i32, i32)> =
            self.single_tiles.difference(&blocked).copied().collect();
        let mut path = router.find_path_to_tile(
            &DoorMap::new(base, &single, None),
            level,
            start_x,
            start_z,
            dest_x,
            dest_z,
        );
        if path.is_empty() {
            let all: HashSet<(i32, i32, i32)> = self
                .door_tiles
                .keys()
                .copied()
                .filter(|tile| !blocked.contains(tile))
                .collect();
            path = router.find_path_to_tile(
                &DoorMap::new(base, &all, None),
                level,
                start_x,
                start_z,
                dest_x,
                dest_z,
            );
        }
        if path.is_empty() {
            return None;
        }
        let steps = expand_waypoints(&path, start_x, start_z);
        let blocked_set: HashSet<usize> = blocked_doors.iter().copied().collect();
        let mut actions = Vec::new();
        let mut seen = HashSet::new();
        for (index, (step_x, step_z)) in steps.iter().enumerate() {
            let Some(door) = self.door_tiles.get(&(*step_x, *step_z, level)) else {
                continue;
            };
            if blocked_set.contains(door) || !is_closed(*door) || !seen.insert(*door) {
                continue;
            }
            let spot = &self.doors[*door];
            let meta = self.locs.get(&spot.loc_id);
            actions.push((
                index,
                DoorAction {
                    x: spot.x,
                    z: spot.z,
                    level: spot.level,
                    loc_id: spot.loc_id,
                    name: meta.map(|meta| meta.name.clone()).unwrap_or_default(),
                    op: meta.and_then(|meta| meta.open_op),
                },
            ));
        }
        Some(Plan {
            steps,
            actions,
            target: (dest_x, dest_z),
        })
    }
}

fn footprint(spot: &DoorSpot) -> Vec<(i32, i32, i32)> {
    if spot.shape <= 3 {
        return vec![(spot.x, spot.z, spot.level)];
    }
    // shortcut: read the painted footprint back with a tight box scan
    // (walls touch +-1, locs cover width x length); exact for door-sized locs.
    let reach = spot.width.max(spot.length).max(1);
    let mut probe = CollisionMap::new();
    collision::change_loc_collision(
        &mut probe,
        spot.shape,
        spot.angle,
        spot.blockrange,
        spot.length,
        spot.width,
        spot.active,
        spot.x,
        spot.z,
        spot.level,
        true,
    );
    let mut tiles = Vec::new();
    for tile_x in (spot.x - 1)..=(spot.x + reach) {
        for tile_z in (spot.z - 1)..=(spot.z + reach) {
            if probe.has(tile_x, tile_z, spot.level)
                && probe.get(tile_x, tile_z, spot.level) != collision::OPEN
            {
                tiles.push((tile_x, tile_z, spot.level));
            }
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionMap;

    #[test]
    fn expand_steps_tile_by_tile() {
        let path = vec![
            Waypoint {
                level: 0,
                x: 1002,
                z: 1000,
            },
            Waypoint {
                level: 0,
                x: 1002,
                z: 1002,
            },
        ];
        assert_eq!(
            expand_waypoints(&path, 1000, 1000),
            vec![(1001, 1000), (1002, 1000), (1002, 1001), (1002, 1002)]
        );
    }

    #[test]
    fn door_tile_reads_open_for_planning() {
        let mut base = CollisionMap::new();
        // Solid wall of blocked tiles with one door tile punched through.
        for tile_z in 0..10 {
            base.set(1005, 1000 + tile_z, 0, collision::WALK_BLOCKED);
        }
        base.set(1005, 1005, 0, collision::OPEN);
        let mut doors = HashSet::new();
        doors.insert((1005, 1005, 0));
        let view = DoorMap::new(&base, &doors, None);
        assert_eq!(view.get(1005, 1005, 0), collision::OPEN);
        assert_eq!(view.get(1005, 1004, 0), collision::WALK_BLOCKED);
    }

    #[test]
    fn plan_crosses_open_door() {
        let mut base = CollisionMap::new();
        for tile_x in 990..1015 {
            for tile_z in 990..1015 {
                base.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        // Vertical wall, door footprint at (1005, 1000..1003).
        for tile_z in 995..1010 {
            base.set(1005, tile_z, 0, collision::WALK_BLOCKED);
        }
        for tile_z in 1000..1003 {
            base.set(1005, tile_z, 0, collision::OPEN);
        }
        let doors = vec![DoorSpot {
            x: 1005,
            z: 1001,
            level: 0,
            shape: 0,
            angle: 0,
            blockrange: false,
            length: 1,
            width: 1,
            active: 0,
            loc_id: 7,
        }];
        let mut locs = HashMap::new();
        locs.insert(
            7,
            LocMeta {
                name: "Door".to_string(),
                category: 0,
                open_op: Some(1),
            },
        );
        let planner = Planner::new(doors, locs);
        let mut router = Router::new();
        let closed = |_door: usize| true;
        let plan = planner
            .plan(&mut router, &base, 0, 1000, 1001, 1010, 1001, &closed, &[])
            .expect("door plan must go through");
        assert_eq!(plan.target, (1010, 1001));
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].1.loc_id, 7);
        assert_eq!(plan.actions[0].1.op, Some(1));
    }

    #[test]
    fn empty_plan_without_door_gap() {
        let mut base = CollisionMap::new();
        for tile_x in 990..1015 {
            for tile_z in 990..1015 {
                base.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        for tile_z in 930..1075 {
            base.set(1005, tile_z, 0, collision::WALK_BLOCKED);
        }
        let planner = Planner::new(Vec::new(), HashMap::new());
        let mut router = Router::new();
        let closed = |_door: usize| true;
        // The wall is unbroken: move_near still returns a path to the
        // closest tile, but the far side is unreachable across the window.
        let plan = planner.plan(&mut router, &base, 0, 1000, 1001, 1010, 1001, &closed, &[]);
        if let Some(plan) = plan {
            assert!(
                plan.steps.iter().all(|(x, _)| *x < 1005),
                "no step may cross the solid wall"
            );
        }
    }
}
