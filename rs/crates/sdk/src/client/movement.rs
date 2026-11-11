//! Walk legs: send steps, await arrival, diagnose refusals.
//!
//! The server walks the steps it is sent; `travel_to` (`travel.rs`)
//! plans the route and drives these legs, one postcondition per segment.
//! This module only sends steps and reports what stopped them, never
//! plans, detours or retries.

use lifescape_nav::plan::Tile;

use crate::buffer::Writer;
use crate::client::Bot;
use crate::error::{Error, Result};

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
/// A leg this silent is refused, not slow: 19 live walk legs land at
/// 0.6-6 ticks per tile (median 2), so 13 ticks with zero progress is
/// twice the worst tile gap plus pump margin, never a slow walk.
const STALL_TICKS: u64 = 13;

/// Read-only walk diagnosis, printed by the `debug_walk` script.
#[derive(Debug, Clone)]
pub struct DebugReport {
    pub lines: Vec<String>,
}

impl Bot {
    pub(crate) fn send_and_await(&mut self, leg: &[(i32, i32)], run: bool, deadline: u64, immediate: bool) -> Result<()> {
        if leg.is_empty() {
            return Ok(());
        }
        if let (Some(first), Some(last)) = (leg.first(), leg.last()) {
            self.act(format!("walk leg {} steps ({},{})>({},{})", leg.len(), first.0, first.1, last.0, last.1));
        }
        let (start_x, start_z, start_level) = (self.world.x, self.world.z, self.world.level);
        let outcome = match self.send_walk(leg, run, immediate) {
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
                let flags = self
                    .tile_flags_best_effort(&[(step_x, step_z, self.world.level)])
                    .and_then(|rows| rows.into_iter().next())
                    .map(|row| row.flags);
                self.log(&Self::refused_line(leg, (start_x, start_z, start_level), live, flags));
            }
        }
        outcome
    }

    /// One refusal line, pure so tests lock it: the first step tile carries
    /// the prime suspect (no progress means step one never succeeded), the
    /// live loc names a re-closed door and the flags a blocking wall.
    /// `None` flags mean the static batch was down: the line keeps its
    /// live half instead of inventing a wall.
    pub(crate) fn refused_line(
        leg: &[(i32, i32)],
        start: (i32, i32, i32),
        live: Option<(u16, u8, u8)>,
        flags: Option<u32>,
    ) -> String {
        let live_text = match live {
            Some((loc_id, shape, angle)) => format!("loc {loc_id} shape={shape} angle={angle}"),
            None => "none".to_string(),
        };
        let flags_text = match flags {
            Some(flags) => format!("{flags:#x}"),
            None => "unknown".to_string(),
        };
        format!(
            "step refused: wanted {leg:?} from ({},{},{}) live={live_text} flags={flags_text}",
            start.0, start.1, start.2
        )
    }

    /// First refusal wins: the live scan names the first live loc or known
    /// door in leg order (entities never count — the server walks through
    /// NPCs and players), then one service batch covers the static
    /// shut-step check on a scratch map (never the worker map, which stays
    /// empty). `None` means clean or service down: the caller degrades to
    /// its live-only line. Pure diagnosis, no movement.
    pub(crate) fn suspect_tile(&mut self, leg: &[(i32, i32)], start: (i32, i32, i32)) -> Option<String> {
        let level = start.2;
        for (index, tile) in leg.iter().enumerate() {
            let (tile_x, tile_z) = *tile;
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
        }
        let tiles: Vec<(i32, i32, i32)> = std::iter::once((start.0, start.1, level))
            .chain(leg.iter().map(|(tile_x, tile_z)| (*tile_x, *tile_z, level)))
            .collect();
        let rows = self.tile_flags_best_effort(&tiles)?;
        let mut scratch = crate::collision::CollisionMap::new();
        for row in &rows {
            scratch.set(row.tile.x, row.tile.z, row.tile.level, row.flags);
        }
        let mut prev = (start.0, start.1);
        for (index, tile) in leg.iter().enumerate() {
            let (tile_x, tile_z) = *tile;
            let (dx, dz) = (tile_x - prev.0, tile_z - prev.1);
            if (dx == 0 || dz == 0)
                && !crate::collision::step_open(&scratch, prev.0, prev.1, level, dx, dz)
            {
                let here = scratch.get(prev.0, prev.1, level);
                let there = scratch.get(tile_x, tile_z, level);
                return Some(format!("step {index} shut ({},{})->({tile_x},{tile_z}) flags={here:#x}/{there:#x}", prev.0, prev.1));
            }
            prev = (tile_x, tile_z);
        }
        None
    }

    pub(crate) fn send_walk(&mut self, steps: &[(i32, i32)], run: bool, immediate: bool) -> Result<()> {
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
        let outcome = if immediate {
            self.send_named_now("MOVE_GAMECLICK", &body.bytes())
        } else {
            self.send_named("MOVE_GAMECLICK", &body.bytes())
        };
        outcome
    }

    pub(crate) fn await_leg(&mut self, leg: &[(i32, i32)], deadline: u64) -> Result<()> {
        let Some(target) = leg.last().copied() else {
            return Ok(());
        };
        let mut best = (self.world.x - target.0).abs() + (self.world.z - target.1).abs();
        let mut last_move = self.world.ticks;
        loop {
            let position = (self.world.x, self.world.z);
            if position == target {
                return Ok(());
            }
            let distance = (position.0 - target.0).abs() + (position.1 - target.1).abs();
            // No early pass-through: a leg only ends on its target (the
            // postcondition §6.2 requires per leg). A transient detour
            // keeps walking until it lands; a truly lost leg trips the
            // stall or deadline watch below, never a silent Ok.
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

    /// Diagnose one walk without walking it: the static half (flags,
    /// template locs, plan or NoRoute for the same inputs `travel_to`
    /// uses) comes from the service in one round trip, the live delta
    /// around the target stays local. Options come from the service too
    /// (same versioned `wants` as `travel_to`); without them the report
    /// keeps its live half with the typed `nav-down`.
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
        let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        let dest = Tile { x, z, level };
        // Same options `travel_to` plans with: the served `wants` tags
        // pinned by version (shared versioned cache). Without them the
        // static half reports `nav-down` and only the live half follows.
        match self.nav_options_remote() {
            Ok(options) => match self.debug_snapshot(from, dest, &options) {
                Ok(answer) => {
                lines.push(format!("flags here={:#x} dest={:#x} static=yes", answer.flags_here, answer.flags_dest));
                let mut statics: Vec<(i32, i32, u16, String, u8, u8, String)> = Vec::new();
                for entry in &answer.locs {
                    let name = (self.tables.loc_name)(entry.loc).unwrap_or_else(|| format!("#{}", entry.loc));
                    let extra = answer
                        .defs
                        .iter()
                        .find(|def| def.loc == entry.loc)
                        .map(|def| def.open_op.map(|op| format!("open{op}")).unwrap_or_else(|| "fixed".to_string()))
                        .unwrap_or_else(|| "unknown-def".to_string());
                    statics.push((entry.x, entry.z, entry.loc, name, entry.shape, entry.angle, extra));
                }
                statics.sort();
                lines.push(format!("static locs within 2 of target: {}", statics.len()));
                for (lx, lz, id, name, shape, angle, extra) in statics.iter().take(12) {
                    lines.push(format!("  loc {name} id={id} at ({lx},{lz}) shape={shape} angle={angle} [{extra}]"));
                }
                match answer.outcome {
                    Ok(plan) => {
                        lines.push(format!("reachable in {} segments cost {}", plan.segments.len(), plan.cost));
                        for seg in plan.segments.iter().take(12) {
                            lines.push(format!("  {}", super::travel::seg_summary(seg)));
                        }
                        if let Some(adjusted) = plan.adjusted_dest {
                            lines.push(format!("adjusted_dest ({},{},{})", adjusted.x, adjusted.z, adjusted.level));
                        }
                    }
                    Err(no) => lines.push(format!(
                        "UNREACHABLE {} closest=({},{},{}) blocked=[{}] blockers=[{}]",
                        no.reason.as_str(),
                        no.closest_reachable.x,
                        no.closest_reachable.z,
                        no.closest_reachable.level,
                        no.blocked_by_requirements
                            .iter()
                            .map(|(edge, tags)| format!("{edge:?} missing {tags:?}"))
                            .collect::<Vec<_>>()
                            .join("; "),
                        lifescape_nav::plan::describe_blockers(&no.blockers),
                    )),
                }
            }
                Err(err) => lines.push(format!("static map unavailable: {err}")),
            },
            Err(err) => lines.push(format!("static map unavailable: {err}")),
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
        let mut doors: Vec<(i32, (i32, i32, i32))> = Vec::new();
        let (here_x, here_z) = (self.world.x, self.world.z);
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
    use crate::client::travel::test_nav_context;
    use crate::collision;
    use lifescape_nav::plan::Tile;
    use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};

    /// Refusal line format, locked offline: full chunk debug, start tile,
    /// live loc on the first step (or none), batch flags of that tile —
    /// `unknown` when the batch was down, never an invented wall.
    #[test]
    fn refused_line_format_locked() {
        let shut = Bot::refused_line(&[(3226, 3223), (3225, 3223)], (3226, 3222, 0), Some((1530, 0, 2)), Some(collision::WALK_BLOCKED));
        assert_eq!(
            shut,
            "step refused: wanted [(3226, 3223), (3225, 3223)] from (3226,3222,0) live=loc 1530 shape=0 angle=2 flags=0x240100"
        );
        let open = Bot::refused_line(&[(1, 0)], (0, 0, 0), None, Some(collision::OPEN));
        assert_eq!(open, "step refused: wanted [(1, 0)] from (0,0,0) live=none flags=0x0");
        let down = Bot::refused_line(&[(1, 0)], (0, 0, 0), None, None);
        assert_eq!(down, "step refused: wanted [(1, 0)] from (0,0,0) live=none flags=unknown");
    }

    /// Suspect scan: the live pass names a live loc on step 1 (not leg[0])
    /// with no service involved, and the walk tiles of a real plan read
    /// clean through one service batch. Entities never count (the server
    /// walks through NPCs and players).
    #[test]
    fn suspect_tile_names_true_blocker_not_leg_zero() {
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "suspect".to_string()),
            &test_socket("sdk-suspect"),
            1,
        )
        .expect("service starts");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "suspect"));
        bot.plan_data_version = Some("suspect".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        // Live half, no service needed: step 1 carries the blocker.
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        bot.world.locs.insert((2, 0, 0), Some((99, 10, 0)));
        let found = bot.suspect_tile(&[(1, 0), (2, 0)], (0, 0, 0)).expect("live loc found");
        assert!(found.contains("step 1") && found.contains("live loc 99"), "got: {found}");
        bot.world.locs.remove(&(2, 0, 0));
        // Static half: tiles a real plan walks read clean, one batch.
        bot.world.x = 3228;
        bot.world.z = 3214;
        let (_, plan, _) = bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }).expect("door leg plans");
        let leg: Vec<(i32, i32)> = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                lifescape_nav::plan::Segment::Walk { tiles } if !tiles.is_empty() => {
                    Some(tiles.iter().map(|tile| (tile.x, tile.z)).collect())
                }
                _ => None,
            })
            .expect("door leg starts with a walk");
        assert!(bot.suspect_tile(&leg, (3228, 3214, 0)).is_none(), "planned walk reads clean");
    }

    /// `debug_walk` plans through the service like `travel_to`: same
    /// versioned `wants`, same static half (flags + plan in one trip),
    /// with the live tail after it.
    #[test]
    fn debug_walk_served_matches_remote_plan() {
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "sdk-walk".to_string()),
            &test_socket("sdk-walk"),
            1,
        )
        .expect("service starts");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 3228;
        bot.world.z = 3214;
        bot.world.level = 0;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "walk"));
        bot.plan_data_version = Some("sdk-walk".to_string());
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        let report = bot.debug_walk(3224, 3214, 0);
        let text = report.lines.join("\n");
        assert!(text.contains("reachable in"), "door leg diagnoses reachable, got:\n{text}");
        assert!(text.contains("static=yes"), "static half answered, got:\n{text}");
        assert!(text.contains("live locs within 2"), "live tail follows, got:\n{text}");
    }

    /// `debug_walk` without a service keeps only its live half, with the
    /// same typed `nav-down` as `plan_travel` — never a local plan.
    #[test]
    fn debug_walk_down_is_nav_down_not_a_world_refusal() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 3228;
        bot.world.z = 3214;
        bot.world.level = 0;
        let report = bot.debug_walk(3224, 3214, 0);
        let text = report.lines.join("\n");
        assert!(text.contains("nav-down"), "typed failure, got:\n{text}");
        assert!(text.contains("live locs within 2"), "live half still reported, got:\n{text}");
        assert!(!text.contains("static=yes"), "no static half without the service, got:\n{text}");
    }
}
