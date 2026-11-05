//! Legs: walk_to, approach, single steps and the read-only debug_walk.
//!
//! The server walks the steps it is sent; this router drives it with
//! straight-line legs, one pump per tick, bounded in ticks. It opens no
//! doors and rides no portals: a leg that stalls is reported with
//! position, target and ticks, never silently detoured.
//!
//! shortcut: no local collision map (pack reader not ported), so blocked
//! targets are diagnosed from live state only; add BFS over the collision
//! map when pack loading lands.

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::{Error, Result};
use crate::route::Router;

/// One row of the generated transition table (`lifescape/data/transitions.json`):
/// a stair/ladder/trapdoor tile, where it takes you, and which loc op rides it.
/// Parsed once per session by the worker; walking only rides them.
#[derive(Clone, Debug)]
pub struct TransitionRow {
    pub name: String,
    pub from_tile: (i32, i32, i32),
    pub to_tile: (i32, i32, i32),
    pub loc: u16,
    pub op: u8,
    /// Every tile the loc covers: multi-tile stairs only answer a click on
    /// one of them, and the table's `from` tile is not always that one.
    pub footprint: Vec<(i32, i32)>,
    pub disabled: bool,
}

const STEP_CAP: usize = 24;
/// A leg with no progress for this long is refused, not slow.
const STALL_TICKS: u64 = 100;

/// Read-only walk diagnosis, printed by the `debug_walk` script.
#[derive(Debug, Clone)]
pub struct DebugReport {
    pub lines: Vec<String>,
}

impl Bot {
    fn arrived(&self, x: i32, z: i32, level: i32) -> bool {
        (self.world.x, self.world.z, self.world.level) == (x, z, level)
    }

    /// Walk to an absolute tile, same level, tick-bounded. With a loaded
    /// collision map the route chains 128-tile BFS windows; without one it
    /// falls back to straight-line legs. A stalled leg is retried around a
    /// lateral hop (straight mode) or replanned (routed mode), then
    /// reported with full context. Closed doors fail loudly: door opening
    /// is not ported yet.
    pub fn walk_to(&mut self, x: i32, z: i32, level: i32, timeout: u64) -> Result<()> {
        self.act(format!("walk_to ({x},{z},{level}) timeout={timeout}"));
        if level != self.world.level {
            return Err(Error::Unreachable(format!(
                "walk_to stays on one level (portal routing not ported): target ({x},{z},{level}) from {:?}",
                self.position()
            )));
        }
        if self.maps.is_some() {
            return self.walk_routed(x, z, level, timeout);
        }
        let run = self.want_run();
        let deadline = self.world.ticks + timeout;
        self.route_target = Some((x, z));
        let mut failures = 0u32;
        while !self.arrived(x, z, level) {
            if self.world.ticks > deadline {
                self.route.clear();
                self.route_target = None;
                return Err(Error::Timeout(format!(
                    "walk to {x},{z} timed out at {:?} ({} ticks)",
                    self.position(),
                    timeout
                )));
            }
            let leg = self.straight_chunk(x, z);
            self.route = leg.clone();
            if let Err(err) = self.send_and_await(&leg, run, deadline) {
                if crate::error::is_interrupted(&err) {
                    self.route.clear();
                    self.route_target = None;
                    return Err(err);
                }
                failures += 1;
                if failures >= 3 {
                    self.route.clear();
                    self.route_target = None;
                    return Err(Error::Timeout(format!(
                        "walk to {x},{z} stuck at {:?}: {err} ({} ticks)",
                        self.position(),
                        timeout
                    )));
                }
                // One lateral hop before giving the leg up: slides around
                // small obstacles the straight line clips. An operator
                // stop aborts instead of detouring.
                match self.lateral_hop(x, z, run, deadline) {
                    Err(err) => {
                        self.route.clear();
                        self.route_target = None;
                        return Err(err);
                    }
                    Ok(false) => {
                        self.route.clear();
                        self.route_target = None;
                        return Err(Error::Unreachable(format!(
                            "nothing reachable toward {x},{z} from {:?}: {err}",
                            self.position()
                        )));
                    }
                    Ok(true) => {}
                }
            } else {
                failures = 0;
            }
        }
        self.route.clear();
        self.route_target = None;
        Ok(())
    }

    /// Up to STEP_CAP diagonal steps toward the target.
    fn straight_chunk(&self, x: i32, z: i32) -> Vec<(i32, i32)> {
        let mut chunk = Vec::new();
        let (mut cx, mut cz) = (self.world.x, self.world.z);
        while (cx, cz) != (x, z) && chunk.len() < STEP_CAP {
            cx += (x - cx).signum();
            cz += (z - cz).signum();
            chunk.push((cx, cz));
        }
        chunk
    }

    fn send_and_await(&mut self, leg: &[(i32, i32)], run: bool, deadline: u64) -> Result<()> {
        if leg.is_empty() {
            return Ok(());
        }
        if let (Some(first), Some(last)) = (leg.first(), leg.last()) {
            self.act(format!("walk leg {} steps ({},{})>({},{})", leg.len(), first.0, first.1, last.0, last.1));
        }
        self.send_walk(leg, run)?;
        self.await_leg(leg[leg.len() - 1], deadline)
    }

    /// One perpendicular hop to slide off a clipped corner. Ok(true) when
    /// it moved; Ok(false) when every hop failed; Err only on operator stop.
    fn lateral_hop(&mut self, x: i32, z: i32, run: bool, deadline: u64) -> Result<bool> {
        let (dx, dz) = (x - self.world.x, z - self.world.z);
        for offset in [4, -4, 8, -8] {
            let (hx, hz) = if dx.abs() >= dz.abs() {
                (self.world.x, self.world.z + offset)
            } else {
                (self.world.x + offset, self.world.z)
            };
            let hop = self.straight_chunk(hx, hz);
            if hop.is_empty() {
                continue;
            }
            if self.send_walk(&hop, run).is_err() {
                continue;
            }
            match self.await_leg(hop[hop.len() - 1], deadline) {
                Ok(()) => return Ok(true),
                Err(err) if crate::error::is_interrupted(&err) => return Err(err),
                Err(_) => {}
            }
        }
        Ok(false)
    }

    /// Entry side of a maze walllow door (content `all.loc`
    /// `approach_direction` param): 0 opens from any side, 1 needs the
    /// `check_axis` side, 2 the other. None when not a maze door.
    /// Public for the maze solver's own route planning.
    pub fn maze_entry_side(loc_id: u16) -> Option<u8> {
        match loc_id {
            3628 => Some(0), // macro_maze_walllow
            3629 => Some(1), // macro_maze_walllow_safe1
            3630 => Some(2), // macro_maze_walllow_safe2
            3631 => Some(2), // macro_maze_walllow_safe3
            3632 => Some(1), // macor_maze_walllow_safe4
            _ => None,
        }
    }

    /// True for the five maze walllow doors (content `all.loc`, category
    /// `macro_maze_wall_door`), the only maze locs that swing.
    pub fn is_maze_door(loc_id: u16) -> bool {
        Self::maze_entry_side(loc_id).is_some()
    }

    /// Stand on a cardinal tile of a maze door, on its entry side: the
    /// server refuses wall clicks from diagonals ("can't reach that")
    /// and one-way doors from the wrong side, and a click from the wrong
    /// side only bounces back onto the door. True once standing on one.
    /// Walks explicit BFS steps (capped): walk_to's fan would wander off
    /// opening random doors instead of stepping next door.
    fn approach_maze_door(
        &mut self,
        door_x: i32,
        door_z: i32,
        door_level: i32,
        loc_id: u16,
        angle: u8,
        _timeout: u64,
    ) -> bool {
        let entry = Self::maze_entry_side(loc_id).unwrap_or(0);
        // check_axis (door_procs.rs2): W/E doors read the same-X tiles,
        // N/S doors the same-Z ones. Only the entry side can swing
        // through (entry 2 needs the off side, everything else the axis).
        let axis: Vec<(i32, i32)> = match angle {
            0 | 2 => vec![(door_x, door_z + 1), (door_x, door_z - 1)],
            _ => vec![(door_x + 1, door_z), (door_x - 1, door_z)],
        };
        let off: Vec<(i32, i32)> = match angle {
            0 | 2 => vec![(door_x + 1, door_z), (door_x - 1, door_z)],
            _ => vec![(door_x, door_z + 1), (door_x, door_z - 1)],
        };
        let mut cands = if entry == 2 { off } else { axis };
        cands.sort_by_key(|tile| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs());
        self.ensure_area(self.world.x, self.world.z, 70);
        let planning = match &self.maps {
            Some(pack) => pack.planning_view(&self.collision, &self.doors, &self.toll_gates, self.allow_paid),
            None => self.collision.clone(),
        };
        for (stand_x, stand_z) in cands {
            if self.world.level == door_level && (self.world.x, self.world.z) == (stand_x, stand_z) {
                return true;
            }
            let mut router = Router::new();
            let waypoints = router.find_path_to_tile(&planning, door_level, self.world.x, self.world.z, stand_x, stand_z);
            if waypoints.is_empty() {
                eprintln!("maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): no planned path");
                continue;
            }
            let steps = crate::nav::expand_waypoints(&waypoints, self.world.x, self.world.z);
            if steps.len() > 12 {
                eprintln!("maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): path too long", );
                continue;
            }
            let _ = self.walk_path(&steps, 60);
            eprintln!("maze approach door ({door_x},{door_z}) stand ({stand_x},{stand_z}): walked to {:?}", self.position());
            if self.world.level == door_level && (self.world.x, self.world.z) == (stand_x, stand_z) {
                return true;
            }
        }
        // Exact stands can read shut in live collision while a plain
        // neighbour is one step away: the server walks the last step
        // itself on click (p_walk in macro_event_maze.rs2), so any
        // cardinal tile clicks. Prefer the entry side for one-way doors.
        let mut neighbours = vec![
            (door_x + 1, door_z),
            (door_x - 1, door_z),
            (door_x, door_z + 1),
            (door_x, door_z - 1),
        ];
        neighbours.sort_by_key(|(stand_x, stand_z)| {
            let side = match angle {
                0 | 2 => *stand_x == door_x,
                _ => *stand_z == door_z,
            };
            let wanted = entry == 0 || (entry == 1 && side) || (entry == 2 && !side);
            ((!wanted) as u8, (stand_x - self.world.x).abs() + (stand_z - self.world.z).abs())
        });
        for (stand_x, stand_z) in neighbours {
            if self.world.level == door_level && (self.world.x, self.world.z) == (stand_x, stand_z) {
                return true;
            }
            if self.world.level == door_level
                && (self.world.x - stand_x).abs() + (self.world.z - stand_z).abs() == 1
                && crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, stand_x - self.world.x, stand_z - self.world.z)
                && self.step_to(stand_x, stand_z, 8).unwrap_or(false)
            {
                return true;
            }
        }
        // Already cardinally adjacent (or on the door): click from here.
        if self.world.level == door_level && (self.world.x - door_x).abs() + (self.world.z - door_z).abs() <= 1 {
            return true;
        }
        false
    }

    /// Stand to click a door: maze doors need their correct cardinal
    /// side, every other door takes the generic adjacent approach.
    fn open_door_stand(
        &mut self,
        door_x: i32,
        door_z: i32,
        door_level: i32,
        loc_id: u16,
        angle: u8,
        timeout: u64,
    ) -> bool {
        if Self::is_maze_door(loc_id) {
            self.approach_maze_door(door_x, door_z, door_level, loc_id, angle, timeout)
        } else {
            self.approach(door_x, door_z, door_level, 1, timeout).is_ok()
        }
    }

    /// Nearest known maze walllow door within `radius` tiles, if any.
    pub fn nearest_maze_door(&self, radius: i32) -> Option<(i32, i32, i32)> {
        let mut best: Option<(i32, (i32, i32, i32))> = None;
        for (tile, (loc_id, _, _)) in self.doors.iter() {
            if tile.2 != self.world.level || !Self::is_maze_door(*loc_id) {
                continue;
            }
            let dist = (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs();
            if dist <= radius && best.is_none_or(|known| dist < known.0) {
                best = Some((dist, *tile));
            }
        }
        best.map(|(_, tile)| tile)
    }

    /// Open one maze walllow door: stand on its entry side, click, and if
    /// the swing parks us on the door tile, click once more from there
    /// (standing on the door reads as the through side, so the second
    /// click always teleports through). True when the server responded
    /// (parked or through: either way replan from the new tile); only a
    /// door that leaves us exactly in place reads shut and blacklists.
    pub fn open_maze_door(&mut self, door_x: i32, door_z: i32, door_level: i32) -> Result<bool> {
        let Some((loc_id, shape, angle)) = self.doors.remove(&(door_x, door_z, door_level)) else {
            return Err(Error::Unreachable(format!("no known door at ({door_x},{door_z},{door_level})")));
        };
        if Self::maze_entry_side(loc_id).is_none() {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return Err(Error::Unreachable(format!("loc {loc_id} at ({door_x},{door_z}) is not a maze door")));
        }
        let Some(op) = self.maps.as_ref().and_then(|pack| pack.loc_def(loc_id as usize)).and_then(|def| def.open_op) else {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return Ok(false);
        };
        if !self.approach_maze_door(door_x, door_z, door_level, loc_id, angle, 800) {
            println!("maze door {loc_id} at ({door_x},{door_z}) unreachable from {:?}", self.position());
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return Ok(false);
        }
        println!("opening maze door loc {loc_id} at ({door_x},{door_z},{door_level})");
        self.intend(&format!("opening the maze door at ({door_x},{door_z})"));
        let start = (self.world.x, self.world.z);
        // Through-destination of the swing (door_open for straight walls).
        let dest = match (shape, angle) {
            (0, 0) => (door_x - 1, door_z),
            (0, 1) => (door_x, door_z + 1),
            (0, 2) => (door_x + 1, door_z),
            (0, 3) => (door_x, door_z - 1),
            _ => (door_x, door_z),
        };
        // Up to three clicks: an off-side first click parks us on the
        // door tile, and the swing turns the loc to inviswall for a few
        // ticks (open_and_close_door) — a click inside that window is
        // ignored, so wait for the walllow back before clicking again.
        for _ in 0..3 {
            let before = (self.world.x, self.world.z);
            if self.op_loc(op, door_x, door_z, loc_id).is_err() {
                break;
            }
            let target = self.world.ticks + 6;
            while self.world.ticks < target {
                if self.pump(1).is_err() {
                    break;
                }
                let now = (self.world.x, self.world.z);
                if now == dest && now != before {
                    self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                    return Ok(true);
                }
                if now == (door_x, door_z) {
                    break;
                }
            }
            if (self.world.x, self.world.z) != (door_x, door_z) {
                break;
            }
            let revert = self.world.ticks + 8;
            while self.world.ticks < revert {
                if self.pump(1).is_err() {
                    break;
                }
                if matches!(self.world.loc_at(door_x, door_z, door_level), Some((id, _, _)) if id == loc_id) {
                    break;
                }
            }
        }
        self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
        if (self.world.x, self.world.z) != start {
            // Parked or through without matching the computed dest (stale
            // angle): the door responded, replan from here instead of
            // blacklisting a working door.
            return Ok(true);
        }
        println!("maze door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut");
        if self.world.modal.as_ref().is_some_and(|modal| modal.layer != "overlay") {
            let _ = self.close_modal();
            let _ = self.pump(2);
        }
        Ok(false)
    }

    /// Open the nearest known door that sits ON the planned path: a route
    /// may cross a door ten tiles ahead, and retrying the same refused step
    /// while standing next to nothing is a dead end. Each door is tried once.
    /// The click is verified live (id/shape flip): an unverified door stays
    /// shut in the plan and never resets the stall counter.
    fn open_door_on_path(&mut self, tiles: &[(i32, i32)]) -> bool {
        let mut best: Option<(usize, (i32, i32, i32))> = None;
        for (index, (tile_x, tile_z)) in tiles.iter().enumerate() {
            let key = (*tile_x, *tile_z, self.world.level);
            if !self.doors.contains_key(&key) {
                continue;
            }
            if !self.allow_paid && self.toll_gates.contains(&key) {
                continue;
            }
            if best.as_ref().is_none_or(|(known, _)| index < *known) {
                best = Some((index, key));
            }
        }
        let Some((_, (door_x, door_z, door_level))) = best else {
            return false;
        };
        let Some((loc_id, shape, angle)) = self.doors.remove(&(door_x, door_z, door_level)) else {
            return false;
        };
        let tolled = self.toll_gates.contains(&(door_x, door_z, door_level));
        let Some(op) = self.maps.as_ref()
            .and_then(|pack| pack.loc_def(loc_id as usize))
            .and_then(|def| def.open_op) else {
            return false;
        };
        if self.open_door_stand(door_x, door_z, door_level, loc_id, angle, 800) {
        } else {
            println!("door {loc_id} at ({door_x},{door_z}) unreachable from {:?}", self.position());
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        println!("opening door loc {loc_id} on path at ({door_x},{door_z},{door_level})");
        self.intend(&format!("opening the door at ({door_x},{door_z})"));
        if self.op_loc(op, door_x, door_z, loc_id).is_err() {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        let _ = self.pump(10);
        if tolled && self.allow_paid && self.world.modal.is_some() {
            let _ = self.chat_choose("Yes, ok.");
            let _ = self.pump(10);
        }
        if !self.door_opened(door_x, door_z, door_level, loc_id) {
            println!("door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut");
            if self.world.modal.is_some() {
                let _ = self.close_modal();
                let _ = self.pump(2);
            }
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return false;
        }
        // Maze walllow doors re-lock behind us: keep them known (see
        // open_nearest_door), only lasting doors leave the plan.
        if Self::is_maze_door(loc_id) {
            self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
            return true;
        }
        if let Some(pack) = &self.maps {
            pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
        }
        true
    }

    /// True once the live loc stops showing the closed id (doors flip id,
    /// shape or vanish when they swing). Bounded watch: a click the engine
    /// ignored reads as shut, never as open.
    fn door_opened(&mut self, door_x: i32, door_z: i32, door_level: i32, closed_id: u16) -> bool {
        let target = self.world.ticks + 30;
        while self.world.ticks < target {
            match self.world.loc_at(door_x, door_z, door_level) {
                Some((id, _, _)) if id == closed_id => {}
                _ => return true,
            }
            if self.pump(2).is_err() {
                return false;
            }
        }
        !matches!(
            self.world.loc_at(door_x, door_z, door_level),
            Some((id, _, _)) if id == closed_id
        )
    }

    /// Click open the nearest closed door within `radius` tiles and unpaint
    /// it from the live collision map. True when a door opened for real
    /// (verified live): the caller replans and walks through. Each door is
    /// tried once (re-clicking would shut it again).
    fn open_nearest_door(&mut self, radius: i32) -> bool {
        let mut cands: Vec<(i32, (i32, i32, i32))> = Vec::new();
        for (tile, _) in self.doors.iter() {
            if tile.2 != self.world.level {
                continue;
            }
            if !self.allow_paid && self.toll_gates.contains(tile) {
                continue;
            }
            let dist = (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs();
            if dist <= radius {
                cands.push((dist, *tile));
            }
        }
        cands.sort();
        for (_, (door_x, door_z, door_level)) in cands {
            let Some((loc_id, shape, angle)) = self.doors.remove(&(door_x, door_z, door_level)) else {
                continue;
            };
            let tolled = self.toll_gates.contains(&(door_x, door_z, door_level));
            let Some(op) = self.maps.as_ref()
                .and_then(|pack| pack.loc_def(loc_id as usize))
                .and_then(|def| def.open_op) else {
                continue;
            };
            println!("opening door loc {loc_id} at ({door_x},{door_z},{door_level})");
            self.intend(&format!("opening the door at ({door_x},{door_z})"));
            if self.open_door_stand(door_x, door_z, door_level, loc_id, angle, 300) {
            } else {
                self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                continue;
            }
            if self.op_loc(op, door_x, door_z, loc_id).is_err() {
                self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                continue;
            }
            let _ = self.pump(10);
            if tolled && self.allow_paid && self.world.modal.is_some() {
                let _ = self.chat_choose("Yes, ok.");
                let _ = self.pump(10);
            }
            if !self.door_opened(door_x, door_z, door_level, loc_id) {
                println!("door loc {loc_id} at ({door_x},{door_z}) did not open, leaving it shut");
                if self.world.modal.is_some() {
                    let _ = self.close_modal();
                    let _ = self.pump(2);
                }
                self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                continue;
            }
            // Maze walllow doors re-lock behind us: keep them known so the
            // next crossing can swing them again instead of planning
            // through a tile the server reads shut.
            if Self::is_maze_door(loc_id) {
                self.doors.insert((door_x, door_z, door_level), (loc_id, shape, angle));
                return true;
            }
            if let Some(pack) = &self.maps {
                pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
            }
            return true;
        }
        false
    }

    /// BFS-windowed walk through a loaded collision map. Each window plans
    /// up to ~60 tiles toward the target and walks the expanded steps;
    /// unreachable windows fail loudly with the closest tile and blockers.
    fn walk_routed(&mut self, x: i32, z: i32, level: i32, timeout: u64) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        self.route_target = Some((x, z));
        let run = self.want_run();
        let mut router = Router::new();
        let mut still = 0u32;
        let mut hops = 0u32;
        let mut doors_opened = 0u32;
        let mut best_reach = i32::MAX;
        let mut detours = 0u32;
        let mut visits: std::collections::HashMap<(i32, i32), u32> = std::collections::HashMap::new();
        while !self.arrived(x, z, level) {
            if self.world.ticks > deadline || hops > 120 {
                self.route.clear();
                self.route_target = None;
                return Err(Error::Timeout(format!("walk to {x},{z} timed out at {:?} ({} ticks)", self.position(), timeout)));
            }
            hops += 1;
            let (src_x, src_z) = (self.world.x, self.world.z);
            // Pacing the same tiles means the plans flip without converging:
            // stop loud on the third visit instead of burning the budget.
            let seen = visits.entry((src_x, src_z)).or_insert(0);
            *seen += 1;
            if *seen >= 3 {
                self.route.clear();
                self.route_target = None;
                return Err(Error::Unreachable(format!(
                    "walking in circles around ({src_x},{src_z}) to {x},{z} from {:?}",
                    self.position()
                )));
            }
            // Load the planning window, then work on a private copy so the
            // walk below can pump and repaint freely. Known doors read open
            // for planning; the walker clicks the ones it must cross.
            self.ensure_area(src_x, src_z, 70);
            self.ensure_area(x, z, 10);
            let collision = match &self.maps {
                Some(pack) => pack.planning_view(&self.collision, &self.doors, &self.toll_gates, self.allow_paid),
                None => self.collision.clone(),
            };
            // March toward the target in short hops: each hop plans inside
            // its own window, so detours (bridges, gates) within ~40 tiles
            // are found instead of demanding one window for the whole trek.
            let (dx, dz) = (x - src_x, z - src_z);
            // Fan of hop targets: the direct step first, then 16 bearings at
            // two radii, most promising (closest to the destination) first.
            // A single window cannot solve a whole trek: bridges, gates and
            // castle walls force detours the fan discovers step by step.
            let mut candidates: Vec<(i32, i32)> = Vec::new();
            let direct_x = dx.clamp(-55, 55);
            let direct_z = dz.clamp(-55, 55);
            if (direct_x, direct_z) != (0, 0) {
                candidates.push((direct_x, direct_z));
            }
            for radius in [40.0f64, 80.0] {
                for step in 0..16 {
                    let angle = std::f64::consts::TAU * f64::from(step) / 16.0;
                    let hop_x = src_x + (radius * angle.cos()).round() as i32;
                    let hop_z = src_z + (radius * angle.sin()).round() as i32;
                    if (hop_x, hop_z) != (src_x, src_z) {
                        candidates.push((hop_x - src_x, hop_z - src_z));
                    }
                }
            }
            candidates.sort_by_key(|(hop_dx, hop_dz)| {
                let to_x = x - (src_x + hop_dx);
                let to_z = z - (src_z + hop_dz);
                to_x.abs() + to_z.abs()
            });
            // Detours are allowed but bounded: leaving a courtyard or a
            // walled yard costs a few hops that do not get closer, while a
            // run of them means the target is genuinely unreachable.
            let mut hopped: Option<Vec<(i32, i32)>> = None;
            let mut chosen = (0, 0);
            for (hop_dx, hop_dz) in candidates {
                let tile_x = src_x + hop_dx;
                let tile_z = src_z + hop_dz;
                let waypoints = router.find_path_to_tile(&collision, level, src_x, src_z, tile_x, tile_z);
                if !waypoints.is_empty() {
                    hopped = Some(crate::nav::expand_waypoints(&waypoints, src_x, src_z));
                    chosen = (hop_dx, hop_dz);
                    break;
                }
            }
            if hopped.is_some() {
                let reach = (x - (src_x + chosen.0)).abs() + (z - (src_z + chosen.1)).abs();
                if reach < best_reach {
                    best_reach = reach;
                    detours = 0;
                } else {
                    detours += 1;
                    if detours > 10 {
                        self.route.clear();
                        self.route_target = None;
                        return Err(Error::Unreachable(format!(
                            "no route to {x},{z} from {:?}: still {reach} tiles away after {} detours",
                            self.position(),
                            detours
                        )));
                    }
                }
            }
            let tiles = match hopped {
                Some(tiles) if !tiles.is_empty() => tiles,
                _ => {
                    if doors_opened < 8 && self.open_nearest_door(15) {
                        doors_opened += 1;
                        continue;
                    }
                    let report = router.unreachable_report(
                        &collision, level, src_x, src_z, x, z, None, 8, 1, 1, 0, crate::reach::SHAPE_TILE, 0,
                    );
                    self.route.clear();
                    self.route_target = None;
                    return Err(Error::Unreachable(format!(
                        "no route to {x},{z} from {:?}: nearest={:?} blocked={} blockers={:?}",
                        self.position(), report.nearest, report.dest_blocked, report.blockers,
                    )));
                }
            };
            let final_hop = tiles.last() == Some(&(x, z));
            self.route = tiles.clone();
            let goal = tiles.last().copied();
            for chunk in tiles.chunks(STEP_CAP) {
                if self.world.ticks > deadline {
                    self.route.clear();
                    self.route_target = None;
                    return Err(Error::Timeout(format!("walk to {x},{z} timed out at {:?} ({} ticks)", self.position(), timeout)));
                }
                if let Err(err) = self.send_and_await(chunk, run, deadline) {
                    if crate::error::is_interrupted(&err) {
                        self.route.clear();
                        self.route_target = None;
                        return Err(err);
                    }
                    break;
                }
            }
            if self.arrived(x, z, level) {
                break;
            }
            // Final hop that ends somewhere else: the target tile itself
            // is blocked (bank booth, tree, counter). The caller checks
            // its own tolerance (approach radius). Anything else that
            // made no progress counts toward the stall budget.
            if final_hop && goal.is_some_and(|tile| (self.world.x, self.world.z) == tile) {
                break;
            }
            if (self.world.x, self.world.z) == (src_x, src_z) {
                if std::env::var("WALK_DEBUG").is_ok() {
                    eprintln!("walk stalled at {:?} (still={still})", self.position());
                }
                if doors_opened < 8 && self.open_nearest_door(3) {
                    doors_opened += 1;
                    still = 0;
                    continue;
                }
                // Nothing next to us: the refused step may need a door
                // further along the route.
                if doors_opened < 12 && self.open_door_on_path(&self.route.clone()) {
                    doors_opened += 1;
                    still = 0;
                    continue;
                }
                still += 1;
            } else {
                still = 0;
            }
            if still >= 3 {
                self.route.clear();
                self.route_target = None;
                return Err(Error::Timeout(format!("walk to {x},{z} stalled at {:?} ({} ticks)", self.position(), timeout)));
            }
        }
        self.route.clear();
        self.route_target = None;
        Ok(())
    }

    /// Walk within `radius` of a tile (blocked targets count as reached).
    pub fn approach(&mut self, x: i32, z: i32, level: i32, radius: i32, timeout: u64) -> Result<()> {
        self.act(format!("approach ({x},{z},{level}) r={radius} timeout={timeout}"));
        if self.world.level == level && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius {
            return Ok(());
        }
        let dist = (self.world.x - x).abs() + (self.world.z - z).abs();
        let exact_budget = timeout.min((dist as u64 * 50).max(750));
        let mut cause = String::new();
        if let Err(err) = self.walk_to(x, z, level, exact_budget) {
            if crate::error::is_interrupted(&err) {
                return Err(err);
            }
            cause = format!(" exact: {err}");
        }
        if self.world.level == level && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius {
            return Ok(());
        }
        let deadline = self.world.ticks + timeout;
        let here = (self.world.x, self.world.z);
        let mut candidates = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                candidates.push((x + dx, z + dz));
            }
        }
        candidates.sort_by_key(|tile| (tile.0 - here.0).abs() + (tile.1 - here.1).abs());
        for (cx, cz) in candidates {
            if (cx, cz) == here && self.world.level == level {
                return Ok(());
            }
            let rest = deadline.saturating_sub(self.world.ticks).max(250);
            match self.walk_to(cx, cz, level, rest) {
                Err(err) if crate::error::is_interrupted(&err) => return Err(err),
                Err(err) if cause.is_empty() => {
                    cause = format!(" via ({cx},{cz}): {err}");
                }
                Ok(())
                    if self.world.level == level
                        && (self.world.x - x).abs() + (self.world.z - z).abs() <= radius =>
                {
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(Error::Unreachable(format!("nothing reachable within {radius} of {x},{z} from {:?}.{cause}", self.position())))
    }

    /// Stand on an orthogonally adjacent tile of an NPC: the server only
    /// accepts NPC clicks from a side, never from a diagonal.
    pub fn approach_npc(&mut self, slot: u16, timeout: u64) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            let Some(npc) = self.world.npcs.get(&slot) else {
                return Err(Error::Unreachable(format!("NPC {slot} left view at {:?}", self.position())));
            };
            if npc.level != self.world.level {
                return Err(Error::Unreachable(format!("NPC {slot} is on level {} at {:?}", npc.level, self.position())));
            }
            if (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs() == 1 {
                return Ok(());
            }
            let size = (self.tables.npc_size)(npc.npc_type).max(1) as i32;
            let mut ring = Vec::new();
            for dz in 0..size {
                ring.push((npc.x - 1, npc.z + dz));
                ring.push((npc.x + size, npc.z + dz));
            }
            for dx in 0..size {
                ring.push((npc.x + dx, npc.z - 1));
                ring.push((npc.x + dx, npc.z + size));
            }
            ring.sort_by_key(|tile| (tile.0 - self.world.x).abs() + (tile.1 - self.world.z).abs());
            let mut moved = false;
            for (cx, cz) in ring {
                if self.world.ticks >= deadline {
                    break;
                }
                if (cx, cz) == (self.world.x, self.world.z) {
                    continue;
                }
                let rest = deadline.saturating_sub(self.world.ticks).max(250);
                if self.walk_to(cx, cz, self.world.level, rest).is_ok() {
                    moved = true;
                    break;
                }
            }
            if !moved {
                self.pump(1)?;
            }
            if let Some(npc) = self.world.npcs.get(&slot) {
                if npc.level == self.world.level && (npc.x - self.world.x).abs() + (npc.z - self.world.z).abs() == 1 {
                    return Ok(());
                }
            }
        }
        Err(Error::Unreachable(format!("no clickable tile for NPC {slot} from {:?}", self.position())))
    }

    /// Walk one adjacent tile, without routing. True when the server moved us.
    pub fn step_to(&mut self, x: i32, z: i32, timeout: u64) -> Result<bool> {
        if (self.world.x, self.world.z) == (x, z) {
            return Ok(true);
        }
        if (x - self.world.x).abs() + (z - self.world.z).abs() != 1 {
            return Err(Error::Unreachable(format!("step_to is one tile, not {:?} -> {x},{z}", self.position())));
        }
        let run = self.want_run();
        self.send_walk(&[(x, z)], run)?;
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            self.pump(1)?;
            if (self.world.x, self.world.z) == (x, z) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Walk an explicit tile route, no router involved. True at the last tile.
    pub fn walk_steps(&mut self, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
        if steps.is_empty() {
            return Ok(true);
        }
        let run = self.want_run();
        for chunk in steps.chunks(STEP_CAP) {
            self.send_walk(chunk, run)?;
            let deadline = self.world.ticks + timeout.max(1);
            let target = chunk[chunk.len() - 1];
            while self.world.ticks < deadline {
                self.pump(1)?;
                if (self.world.x, self.world.z) == target {
                    break;
                }
            }
        }
        Ok((self.world.x, self.world.z) == steps[steps.len() - 1])
    }

    /// Walk explicit steps one tile at a time, swinging shut maze doors
    /// that block the way. No router, no fan: diagonal steps split into
    /// cardinals (open edge first), a shut edge swings a maze door when
    /// there is one, and anything else stops at once so the caller
    /// replans instead of detouring blindly. True when every step is
    /// done (a teleport mid-route stops early for the same reason).
    pub fn walk_path(&mut self, steps: &[(i32, i32)], timeout: u64) -> Result<bool> {
        let deadline = self.world.ticks + timeout.max(1);
        let mut index = 0;
        while index < steps.len() {
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!(
                    "walk_path timed out at {:?} ({} ticks)",
                    self.position(),
                    timeout
                )));
            }
            let (step_x, step_z) = steps[index];
            if (self.world.x, self.world.z) == (step_x, step_z) {
                index += 1;
                continue;
            }
            let (dx, dz) = (step_x - self.world.x, step_z - self.world.z);
            // Cardinal legs for this step, open edge first.
            let mut legs: Vec<(i32, i32)> = Vec::new();
            if dx != 0 && dz != 0 {
                let x_open = crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, dx.signum(), 0);
                let z_open = crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, 0, dz.signum());
                if x_open {
                    legs.push((dx.signum(), 0));
                    legs.push((0, dz.signum()));
                } else if z_open {
                    legs.push((0, dz.signum()));
                    legs.push((dx.signum(), 0));
                }
            } else if dx != 0 || dz != 0 {
                legs.push((dx.signum(), dz.signum()));
            } else {
                index += 1;
                continue;
            }
            if legs.is_empty() {
                // Corner shut on both edges: swing a maze door there if
                // there is one, then replan from wherever that leaves us.
                for (tile_x, tile_z) in [(self.world.x + dx.signum(), self.world.z), (self.world.x, self.world.z + dz.signum())] {
                    if let Some((loc_id, _, _)) = self.doors.get(&(tile_x, tile_z, self.world.level)).copied() {
                        if Self::is_maze_door(loc_id) {
                            let _ = self.open_maze_door(tile_x, tile_z, self.world.level);
                            break;
                        }
                    }
                }
                return Ok(false);
            }
            let mut advanced = false;
            for (move_x, move_z) in legs {
                if (self.world.x, self.world.z) == (step_x, step_z) {
                    break;
                }
                let (next_x, next_z) = (self.world.x + move_x, self.world.z + move_z);
                eprintln!("maze walk leg ({},{})->({},{}) flags={:#x}", self.world.x, self.world.z, next_x, next_z, self.collision.get(next_x, next_z, self.world.level));
                if !crate::collision::step_open(&self.collision, self.world.x, self.world.z, self.world.level, move_x, move_z) {
                    eprintln!("maze walk leg blocked, door?");
                    if let Some((loc_id, _, _)) = self.doors.get(&(next_x, next_z, self.world.level)).copied() {
                        if Self::is_maze_door(loc_id) {
                            let _ = self.open_maze_door(next_x, next_z, self.world.level);
                        }
                    }
                    break;
                }
                match self.step_to(next_x, next_z, 8) {
                    Ok(true) => {
                        advanced = true;
                    }
                    Ok(false) => {
                        eprintln!("maze walk leg refused");
                        break;
                    }
                    Err(error) if crate::error::is_interrupted(&error) => return Err(error),
                    Err(_) => break,
                }
            }
            if (self.world.x, self.world.z) == (step_x, step_z) {
                index += 1;
            } else if !advanced {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn send_walk(&mut self, steps: &[(i32, i32)], run: bool) -> Result<()> {
        let (start_x, start_z) = (self.world.x, self.world.z);
        for (x, z) in steps {
            // A random-event teleport invalidates the route mid-walk.
            if (x - start_x).abs() > 127 || (z - start_z).abs() > 127 {
                return Err(Error::Unreachable(format!("stale route after teleport at {:?}", self.position())));
            }
        }
        let mut body = Writer::new();
        body.u8(u8::from(run));
        body.u16(start_x as u16);
        body.u16(start_z as u16);
        for (x, z) in steps {
            body.i8((x - start_x) as i8);
            body.i8((z - start_z) as i8);
        }
        self.send_named("MOVE_GAMECLICK", &body.bytes())
    }

    fn await_leg(&mut self, target: (i32, i32), deadline: u64) -> Result<()> {
        let mut best = (self.world.x - target.0).abs() + (self.world.z - target.1).abs();
        let mut last_move = self.world.ticks;
        loop {
            let position = (self.world.x, self.world.z);
            if position == target {
                return Ok(());
            }
            let distance = (position.0 - target.0).abs() + (position.1 - target.1).abs();
            if distance > best {
                return Ok(()); // passed it: the next leg recomputes from here
            }
            if distance < best {
                best = distance;
                last_move = self.world.ticks;
            } else if self.world.ticks.saturating_sub(last_move) > STALL_TICKS {
                return Err(Error::Timeout(format!("leg to {target:?} stalled at {:?}", self.position())));
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("leg to {target:?} timed out at {:?}", self.position())));
            }
            self.pump(1)?;
        }
    }

    /// Diagnose one walk without walking it. Loads the same static windows
    /// the router plans on, then reports static scenery, the live delta
    /// and a BFS verdict (reachable, or nearest tile plus blockers) — the
    /// same inputs walk_to decides on.
    pub fn debug_walk(&mut self, x: i32, z: i32, level: i32) -> DebugReport {
        let mut lines = vec![
            format!("from {:?} to ({x},{z},{level})", self.position()),
            format!(
                "distance manhattan={} chebyshev={}",
                (x - self.world.x).abs() + (z - self.world.z).abs(),
                (x - self.world.x).abs().max((z - self.world.z).abs())
            ),
        ];
        if level != self.world.level {
            lines.push(format!("different level (here {}): walk_to stays on one level", self.world.level));
        }
        let (here_x, here_z) = (self.world.x, self.world.z);
        self.ensure_area(here_x, here_z, 70);
        self.ensure_area(x, z, 10);
        let collision = match &self.maps {
            Some(pack) => pack.planning_view(&self.collision, &self.doors, &self.toll_gates, self.allow_paid),
            None => self.collision.clone(),
        };
        lines.push(format!(
            "flags here={:#x} dest={:#x} maps={}",
            collision.get(here_x, here_z, level),
            collision.get(x, z, level),
            if self.maps.is_some() { "yes" } else { "no" },
        ));
        let mut statics: Vec<(i32, i32, u16, String, u8, u8, String)> = Vec::new();
        for ((lx, lz, ll), (id, shape, angle)) in &self.world.static_locs {
            if *ll == level && (lx - x).abs() <= 2 && (lz - z).abs() <= 2 {
                let name = (self.tables.loc_name)(*id).unwrap_or_else(|| format!("#{id}"));
                let extra = self
                    .maps
                    .as_ref()
                    .and_then(|pack| pack.loc_def(*id as usize))
                    .map(|def| {
                        let op = def.open_op.map(|op| format!("open{op}")).unwrap_or_else(|| "fixed".to_string());
                        if def.toll { format!("{op}+toll") } else { op }
                    })
                    .unwrap_or_else(|| "unknown-def".to_string());
                statics.push((*lx, *lz, *id, name, *shape, *angle, extra));
            }
        }
        statics.sort();
        lines.push(format!("static locs within 2 of target: {}", statics.len()));
        for (lx, lz, id, name, shape, angle, extra) in statics.iter().take(12) {
            lines.push(format!("  loc {name} id={id} at ({lx},{lz}) shape={shape} angle={angle} [{extra}]"));
        }
        let mut locs: Vec<(i32, i32, u16, String, u8, u8)> = Vec::new();
        for ((lx, lz, ll), value) in &self.world.locs {
            if *ll == level && (lx - x).abs() <= 2 && (lz - z).abs() <= 2 {
                if let Some((id, shape, angle)) = value {
                    let name = (self.tables.loc_name)(*id)
                        .unwrap_or_else(|| format!("#{id}"));
                    locs.push((*lx, *lz, *id, name, *shape, *angle));
                }
            }
        }
        locs.sort();
        lines.push(format!("live locs within 2 of target: {}", locs.len()));
        for (lx, lz, id, name, shape, angle) in locs.iter().take(12) {
            lines.push(format!("  loc {name} id={id} at ({lx},{lz}) shape={shape} angle={angle}"));
        }
        let mut router = Router::new();
        let waypoints = router.find_path_to_tile(&collision, level, here_x, here_z, x, z);
        if waypoints.is_empty() {
            let report = router.unreachable_report(
                &collision, level, here_x, here_z, x, z, None, 8, 1, 1, 0, crate::reach::SHAPE_TILE, 0,
            );
            lines.push(format!(
                "UNREACHABLE nearest={:?} dest_blocked={} blockers={:?}",
                report.nearest, report.dest_blocked, report.blockers,
            ));
        } else {
            lines.push(format!("reachable in {} waypoints", waypoints.len()));
            let via: Vec<String> = waypoints.iter().take(12).map(|w| format!("({},{})", w.x, w.z)).collect();
            lines.push(format!("via {}", via.join(" ")));
            // Doors the walker would have to swing, in walking order.
            let steps = crate::nav::expand_waypoints(&waypoints, here_x, here_z);
            let mut on_path: Vec<(i32, i32, u16)> = Vec::new();
            for (step_x, step_z) in &steps {
                if let Some((id, _, _)) = self.doors.get(&(*step_x, *step_z, level)) {
                    if on_path.last().is_none_or(|last| last.0 != *step_x || last.1 != *step_z) {
                        on_path.push((*step_x, *step_z, *id));
                    }
                }
            }
            lines.push(format!("steps={} doors_on_path={}", steps.len(), on_path.len()));
            for (door_x, door_z, id) in on_path.iter().take(10) {
                let name = (self.tables.loc_name)(*id).unwrap_or_else(|| format!("#{id}"));
                lines.push(format!("  path door {name} id={id} at ({door_x},{door_z})"));
            }
        }
        let mut doors: Vec<(i32, (i32, i32, i32))> = Vec::new();
        for tile in self.doors.keys() {
            if tile.2 != self.world.level {
                continue;
            }
            let dist = (tile.0 - here_x).abs() + (tile.1 - here_z).abs();
            if dist <= 15 {
                doors.push((dist, *tile));
            }
        }
        doors.sort();
        lines.push(format!("known doors within 15: {}", doors.len()));
        for (_, (door_x, door_z, door_level)) in doors.iter().take(10) {
            let label = self
                .doors
                .get(&(*door_x, *door_z, *door_level))
                .map(|(id, _, _)| (self.tables.loc_name)(*id).unwrap_or_else(|| format!("#{id}")))
                .unwrap_or_default();
            let toll = if self.toll_gates.contains(&(*door_x, *door_z, *door_level)) { "+toll" } else { "" };
            lines.push(format!("  door {label} at ({door_x},{door_z}){toll}"));
        }
        let mut objs: Vec<((i32, i32, i32), u16, u32)> =
            self.world.ground_items(x, z, level, 8, None).into_iter().map(|((tile, oid), count)| (tile, oid, count)).collect();
        objs.sort();
        lines.push(format!("live ground objs within 8 of target: {}", objs.len()));
        for (tile, oid, count) in objs.iter().take(20) {
            let name =
                (self.tables.obj_name)(*oid).unwrap_or_else(|| format!("#{oid}"));
            lines.push(format!("  obj {name} x{count} at ({},{})", tile.0, tile.1));
        }
        let near_npcs = self.world.npcs.values().filter(|npc| npc.level == level && (npc.x - x).abs() + (npc.z - z).abs() <= 8).count();
        lines.push(format!("live npcs within 8 of target: {near_npcs}"));
        DebugReport { lines }
    }
}
