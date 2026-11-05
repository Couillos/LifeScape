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

    /// Open the nearest known door that sits ON the planned path: a route
    /// may cross a door ten tiles ahead, and retrying the same refused step
    /// while standing next to nothing is a dead end. Each door is tried once.
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
        if self.approach(door_x, door_z, door_level, 1, 800).is_err() {
            println!("door {loc_id} at ({door_x},{door_z}) unreachable from {:?}", self.position());
            return false;
        }
        println!("opening door loc {loc_id} on path at ({door_x},{door_z},{door_level})");
        self.intend(&format!("opening the door at ({door_x},{door_z})"));
        if self.op_loc(op, door_x, door_z, loc_id).is_err() {
            return false;
        }
        let _ = self.pump(10);
        if tolled && self.allow_paid && self.world.modal.is_some() {
            let _ = self.chat_choose("Yes, ok.");
            let _ = self.pump(10);
        }
        if let Some(pack) = &self.maps {
            pack.unpaint_door(&mut self.collision, loc_id, shape, angle, door_x, door_z, door_level);
        }
        true
    }

    /// Click open the nearest closed door within `radius` tiles and unpaint
    /// it from the live collision map. True when a door was clicked: the
    /// caller replans and either walks through or picks the next one.
    /// Each door is tried once (re-clicking would shut it again).
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
            if self.approach(door_x, door_z, door_level, 1, 300).is_err() {
                continue;
            }
            if self.op_loc(op, door_x, door_z, loc_id).is_err() {
                continue;
            }
            let _ = self.pump(10);
            if tolled && self.allow_paid && self.world.modal.is_some() {
                let _ = self.chat_choose("Yes, ok.");
                let _ = self.pump(10);
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
        while !self.arrived(x, z, level) {
            if self.world.ticks > deadline || hops > 120 {
                self.route.clear();
                self.route_target = None;
                return Err(Error::Timeout(format!("walk to {x},{z} timed out at {:?} ({} ticks)", self.position(), timeout)));
            }
            hops += 1;
            let (src_x, src_z) = (self.world.x, self.world.z);
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

    /// Diagnose one walk without walking it. Read-only: live locs, objs,
    /// NPCs and players around the target plus a straight-line verdict
    /// (no static collision map in this port).
    pub fn debug_walk(&self, x: i32, z: i32, level: i32) -> DebugReport {
        let mut lines = vec![
            format!("from {:?} to ({x},{z},{level})", self.position()),
            format!(
                "distance manhattan={} chebyshev={}",
                (x - self.world.x).abs() + (z - self.world.z).abs(),
                (x - self.world.x).abs().max((z - self.world.z).abs())
            ),
        ];
        if level != self.world.level {
            lines.push(format!("different level (here {}): portal routing not ported", self.world.level));
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
        for (lx, lz, id, name, shape, angle) in locs.iter().take(25) {
            lines.push(format!("  loc {name} id={id} at ({lx},{lz}) shape={shape} angle={angle}"));
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
        lines.push("static map: not loaded (pack reader not ported); verdict from live state only".to_string());
        DebugReport { lines }
    }
}
