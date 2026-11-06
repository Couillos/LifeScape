//! Walk legs: send steps, await arrival, diagnose refusals.
//!
//! The server walks the steps it is sent; `travel_to` (`travel.rs`)
//! plans the route and drives these legs, one postcondition per segment.
//! This module only sends steps and reports what stopped them, never
//! plans, detours or retries.

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::{Error, Result};
use crate::route::Router;

/// One row of the generated transition table (`gamedata/data/transitions.json`):
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

/// Max steps per walk send: the plan's legs are cut into these chunks.
pub(crate) const STEP_CAP: usize = 24;
/// A leg with no progress this long is refused, not slow: walking moves
/// every couple of ticks, so a silent leg is a refused one.
const STALL_TICKS: u64 = 12;

/// Read-only walk diagnosis, printed by the `debug_walk` script.
#[derive(Debug, Clone)]
pub struct DebugReport {
    pub lines: Vec<String>,
}

impl Bot {
    pub(crate) fn send_and_await(&mut self, leg: &[(i32, i32)], run: bool, deadline: u64) -> Result<()> {
        if leg.is_empty() {
            return Ok(());
        }
        if let (Some(first), Some(last)) = (leg.first(), leg.last()) {
            self.act(format!("walk leg {} steps ({},{})>({},{})", leg.len(), first.0, first.1, last.0, last.1));
        }
        let (start_x, start_z, start_level) = (self.world.x, self.world.z, self.world.level);
        let outcome = match self.send_walk(leg, run) {
            Err(err) => Err(err),
            Ok(()) => self.await_leg(leg, deadline),
        };
        // Refusal probe, read-only: a chunk ending on its start tile moved
        // nowhere, so the server refused the step (or nothing was sent) —
        // never a slow walk. Stops and deadline timeouts report elsewhere.
        let stopped = matches!(&outcome, Err(err) if crate::error::is_interrupted(err));
        if !stopped
            && !matches!(&outcome, Err(Error::Io(_) | Error::Login(_)))
            && self.world.ticks <= deadline
            && (self.world.x, self.world.z, self.world.level) == (start_x, start_z, start_level)
        {
            if let Some(suspect) = self.suspect_tile(leg, (start_x, start_z, start_level)) {
                self.log(&format!("walk blocked: {suspect}"));
            } else {
                let (step_x, step_z) = leg[0];
                let live = self.world.loc_at(step_x, step_z, self.world.level);
                let flags = self.collision.get(step_x, step_z, self.world.level);
                self.log(&Self::refused_line(leg, (start_x, start_z, start_level), live, flags));
            }
        }
        outcome
    }

    /// One refusal line, pure so tests lock it: the first step tile carries
    /// the prime suspect (no progress means step one never succeeded), the
    /// live loc names a re-closed door and the flags a blocking wall.
    pub(crate) fn refused_line(leg: &[(i32, i32)], start: (i32, i32, i32), live: Option<(u16, u8, u8)>, flags: u32) -> String {
        let live_text = match live {
            Some((loc_id, shape, angle)) => format!("loc {loc_id} shape={shape} angle={angle}"),
            None => "none".to_string(),
        };
        format!(
            "step refused: wanted {leg:?} from ({},{},{}) live={live_text} flags={flags:#x}",
            start.0, start.1, start.2
        )
    }

    /// First leg step that explains a refusal: live locs,
    /// directionally-shut steps and known doors. Entities never block
    /// walking server-side (`canMove` ignores `NPC_OCC`/`PLAYER_OCC`),
    /// so occupants are deliberately not reported. The old probe below
    /// only sees leg[0]'s flags, so a blocker at step 2..24 reads as a
    /// refusal on an open tile. Pure diagnosis, no movement.
    pub(crate) fn suspect_tile(&self, leg: &[(i32, i32)], start: (i32, i32, i32)) -> Option<String> {
        let level = start.2;
        let mut prev = (start.0, start.1);
        for (index, tile) in leg.iter().enumerate() {
            let (tile_x, tile_z) = *tile;
            let (dx, dz) = (tile_x - prev.0, tile_z - prev.1);
            // Live ground decor (shape 22: soil, grass, rugs) never blocks
            // feet; reporting it cried wolf on every courtyard stall.
            if let Some((loc, shape, angle)) = self.world.loc_at(tile_x, tile_z, level) {
                if shape != 22 {
                    return Some(format!("step {index} live loc {loc} shape={shape} angle={angle} at ({tile_x},{tile_z})"));
                }
            }
            if self.doors.contains_key(&(tile_x, tile_z, level)) {
                return Some(format!("step {index} known door at ({tile_x},{tile_z})"));
            }
            if (dx == 0 || dz == 0)
                && !crate::collision::step_open(&self.collision, prev.0, prev.1, level, dx, dz)
            {
                let here = self.collision.get(prev.0, prev.1, level);
                let there = self.collision.get(tile_x, tile_z, level);
                return Some(format!("step {index} shut ({},{})->({tile_x},{tile_z}) flags={here:#x}/{there:#x}", prev.0, prev.1));
            }
            prev = (tile_x, tile_z);
        }
        None
    }

    pub(crate) fn send_walk(&mut self, steps: &[(i32, i32)], run: bool) -> Result<()> {
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

    pub(crate) fn await_leg(&mut self, leg: &[(i32, i32)], deadline: u64) -> Result<()> {
        let Some(target) = leg.last().copied() else {
            return Ok(());
        };
        let mut best = (self.world.x - target.0).abs() + (self.world.z - target.1).abs();
        let mut last_move = self.world.ticks;
        let mut door_tried = false;
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
                door_tried = false;
            } else if self.world.ticks.saturating_sub(last_move) > STALL_TICKS {
                return Err(Error::Timeout(format!("leg to {target:?} stalled at {:?}", self.position())));
            } else if !door_tried && self.world.ticks.saturating_sub(last_move) > 12 {
                // Stalled against a door the plan read open: the server
                // re-closes them, so swing it now instead of waiting out
                // the stall budget and replanning through it again.
                door_tried = true;
                let mut shut: Option<(i32, i32, u16)> = None;
                for (step_x, step_z) in leg {
                    if (step_x - position.0).abs() + (step_z - position.1).abs() > 2 {
                        continue;
                    }
                    if let Some((loc_id, _, _)) = self.doors.get(&(*step_x, *step_z, self.world.level)).copied() {
                        if !Self::is_maze_door(loc_id) {
                            shut = Some((*step_x, *step_z, loc_id));
                            break;
                        }
                    }
                }
                if let Some((door_x, door_z, loc_id)) = shut {
                    if self.open_shut_door(door_x, door_z, self.world.level, loc_id) {
                        last_move = self.world.ticks;
                        door_tried = false;
                    } else {
                        return Err(Error::Unreachable(format!(
                            "leg to {target:?} shut at door {loc_id} ({door_x},{door_z}) from {:?}",
                            self.position()
                        )));
                    }
                }
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
            lines.push(format!("different level (here {}): travel plans across levels", self.world.level));
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
                    .map(|def| def.open_op.map(|op| format!("open{op}")).unwrap_or_else(|| "fixed".to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{self};

    /// Refusal line format, locked offline: full chunk debug, start tile,
    /// live loc on the first step (or none), collision flags of that tile.
    #[test]
    fn refused_line_format_locked() {
        let shut = Bot::refused_line(&[(3226, 3223), (3225, 3223)], (3226, 3222, 0), Some((1530, 0, 2)), collision::WALK_BLOCKED);
        assert_eq!(
            shut,
            "step refused: wanted [(3226, 3223), (3225, 3223)] from (3226,3222,0) live=loc 1530 shape=0 angle=2 flags=0x240100"
        );
        let open = Bot::refused_line(&[(1, 0)], (0, 0, 0), None, collision::OPEN);
        assert_eq!(open, "step refused: wanted [(1, 0)] from (0,0,0) live=none flags=0x0");
    }

    /// Suspect scan, pure: a live loc on step 1 (not leg[0]) names step 1,
    /// a clean leg finds nothing, and entities never count (the server
    /// walks through NPCs and players).
    #[test]
    fn suspect_tile_names_true_blocker_not_leg_zero() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        for tile in [(0, 0), (1, 0), (2, 0), (1, 1), (0, 1)] {
            bot.collision.set(tile.0, tile.1, 0, collision::OPEN);
        }
        assert!(bot.suspect_tile(&[(1, 0), (2, 0)], (0, 0, 0)).is_none(), "clean leg");
        bot.world.locs.insert((2, 0, 0), Some((99, 10, 0)));
        let found = bot.suspect_tile(&[(1, 0), (2, 0)], (0, 0, 0)).expect("live loc found");
        assert!(found.contains("step 1") && found.contains("live loc 99"), "got: {found}");
    }
}
