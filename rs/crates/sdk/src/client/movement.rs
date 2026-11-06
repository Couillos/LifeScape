//! Legs: walk_to over planned routes, one pump per tick, bounded in ticks.
//!
//! The server walks the steps it is sent; this router drives it with
//! straight-line legs. Door handling lives in `doors` (open like the manual
//! player does, adjacent and at once); portals are never ridden: a leg that
//! stalls is reported with position, target and ticks, never silently detoured.
//!
//! A blocked target tile (tree, booth, counter) resolves once to its
//! BFS-nearest reachable tile; only a truly unreachable target fails loud.

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
    fn arrived(&self, x: i32, z: i32, level: i32) -> bool {
        (self.world.x, self.world.z, self.world.level) == (x, z, level)
    }

    /// Walk to an absolute tile, same level, tick-bounded. With a loaded
    /// collision map the route chains 128-tile BFS windows; without one it
    /// falls back to straight-line legs. A stalled leg is retried around a
    /// lateral hop (straight mode) or replanned (routed mode), then
    /// reported with full context. Closed doors fail loudly: door opening
    /// is not ported yet.
    ///
    /// Last resort before any routed give-up (`walking in circles`,
    /// `detours`, `stalled`, `no route`) with zero net movement: one
    /// server-truth sidestep in each cardinal direction (`bump_step`).
    /// The planner's collision can read shut where the server still walks
    /// (Draynor market stalls); the server's answer to a direct step is
    /// truth. One bump+replan cycle per call, then the original error.
    /// Walkable targets and passing walks never bump: tick for tick the
    /// old paths.
    ///
    /// Contract: Ok means the exact tile is reached, or — when the target
    /// tile itself collides (tree, booth, counter) — the BFS-nearest
    /// reachable tile (resolved once in `walk_routed`). Walkable targets
    /// behave exactly as before, tick for tick.
    pub fn walk_to(&mut self, x: i32, z: i32, level: i32, timeout: u64) -> Result<()> {
        // Stall detection (a few ticks without progress) reports stuck
        // walks; this overall budget only caps absurd detours, so scale
        // it to the distance instead of trusting caller values.
        let dist = (self.world.x - x).abs() + (self.world.z - z).abs();
        let timeout = timeout.min(dist as u64 * 10 + 300);
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
                if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
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
            if let Err(err) = self.send_walk(&hop, run) {
                if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                    return Err(err);
                }
                continue;
            }
            match self.await_leg(&hop, deadline) {
                Ok(()) => return Ok(true),
                Err(err) if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                Err(_) => {}
            }
        }
        Ok(false)
    }

    /// Entry side of a maze walllow door (content `all.loc`
    /// `approach_direction` param): 0 opens from any side, 1 needs the
    /// `check_axis` side, 2 the other. None when not a maze door.
    /// Public for the maze solver's own route planning.


    /// Bump-escape trigger, pure so tests lock it: only a give-up with
    /// zero net movement (still on the call's entry tile) may sidestep,
    /// and only once per `walk_to` call.
    fn bump_armed(entry: (i32, i32, i32), current: (i32, i32, i32), bumped: bool) -> bool {
        !bumped && entry == current
    }

    /// One server-truth sidestep in each cardinal direction, no planning:
    /// the planner's collision is the suspect, so it is not consulted.
    /// Each step is tick-bounded (4 pumps, `step_to`'s own bound); a refused
    /// step tries the next side. True on the first step the server walks.
    /// Only an operator stop is an Err; every refusal is just the next side.
    fn bump_step(&mut self) -> Result<bool> {
        for (step_x, step_z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (next_x, next_z) = (self.world.x + step_x, self.world.z + step_z);
            match self.step_to(next_x, next_z, 4) {
                Ok(true) => {
                    println!("{} bump stepped to ({next_x},{next_z})", self.tag());
                    return Ok(true);
                }
                Ok(false) => {}
                Err(err) if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) => return Err(err),
                Err(_) => {}
            }
        }
        Ok(false)
    }

    /// Shared by every `walk_routed` give-up: when the planner quits
    /// without net movement and no bump fired yet, try one server-truth
    /// sidestep. Ok(true): the bump moved, replan once (`still` reset,
    /// `bumped` set). Ok(false): still walled in, return the original
    /// error unchanged. Err: operator stop, return it at once.
    fn bump_escape(&mut self, entry: (i32, i32, i32), bumped: &mut bool, still: &mut u32) -> Result<bool> {
        if !Self::bump_armed(entry, (self.world.x, self.world.z, self.world.level), *bumped) {
            return Ok(false);
        }
        match self.bump_step() {
            Ok(true) => {
                *bumped = true;
                *still = 0;
                Ok(true)
            }
            Ok(false) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Hop rank: closeness to the goal first (routing unchanged), then
    /// fewest visits. Detouring back through a walked tile loses ties,
    /// which kills the plan-A/plan-B flip-flop (`walking in circles`)
    /// without banning real backtracks (a lone planned option still
    /// wins whatever its rank).
    pub(crate) fn hop_rank(
        goal_x: i32,
        goal_z: i32,
        src_x: i32,
        src_z: i32,
        hop: (i32, i32),
        visits: &std::collections::HashMap<(i32, i32), u32>,
    ) -> (i32, u32) {
        let (hop_dx, hop_dz) = hop;
        let to_x = goal_x - (src_x + hop_dx);
        let to_z = goal_z - (src_z + hop_dz);
        let seen = visits.get(&(src_x + hop_dx, src_z + hop_dz)).copied().unwrap_or(0);
        (to_x.abs() + to_z.abs(), seen)
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
        // Bump-escape anchor: the entry tile, plus whether the single
        // bump+replan cycle already fired. Only a give-up with zero net
        // movement may bump, and only once.
        let entry = (self.world.x, self.world.z, self.world.level);
        let mut bumped = false;
        let mut visits: std::collections::HashMap<(i32, i32), u32> = std::collections::HashMap::new();
        // Blocked-target goal, resolved once: the exact tile of a tree,
        // booth or counter collides, so `arrived` on it never converges
        // (the walk paced until `walking in circles`). The goal switches to
        // the BFS-nearest reachable tile; walkable targets keep the exact
        // tile, tick for tick as before.
        let mut goal_x = x;
        let mut goal_z = z;
        while !self.arrived(goal_x, goal_z, level) {
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
                // Planner loops without the server refusing a step: the
                // map may read shut where the server walks. One sidestep
                // on the entry tile only; moved walks replan, walled-in
                // walks keep this exact error.
                match self.bump_escape(entry, &mut bumped, &mut still) {
                    Err(err) => {
                        self.route.clear();
                        self.route_target = None;
                        return Err(err);
                    }
                    Ok(true) => continue,
                    Ok(false) => {}
                }
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
            // Load the corridor ahead too: unloaded squares read as walls,
            // so without this only hops back into known territory plan and
            // the fan oscillates instead of advancing.
            self.ensure_area(src_x + (x - src_x).clamp(-55, 55), src_z + (z - src_z).clamp(-55, 55), 45);
            // Live door states into the base map before planning, or the
            // legs route through tiles the server reads shut.
            self.sync_live_doors();
            let collision = match &self.maps {
                Some(pack) => pack.planning_view(&self.collision, &self.doors, &self.toll_gates, self.allow_paid),
                None => self.collision.clone(),
            };
            // First iteration that sees the dest inside the window (at once
            // for a near target): a blocked dest switches the goal to its
            // BFS-nearest reachable tile and finishes Ok there. Still
            // unresolvable (too far for one window, or no visited tile near
            // it) keeps the exact goal and fails loud below with its
            // blockers — never a silent fallback.
            if (goal_x, goal_z) == (x, z) && !crate::collision::tile_walkable(collision.get(x, z, level)) {
                if let Some(near) = router.closest_to_blocked(&collision, level, src_x, src_z, x, z) {
                    goal_x = near.0;
                    goal_z = near.1;
                    if (src_x, src_z) == (goal_x, goal_z) {
                        self.route.clear();
                        self.route_target = None;
                        return Ok(());
                    }
                }
            }
            // March toward the target in short hops: each hop plans inside
            // its own window, so detours (bridges, gates) within ~40 tiles
            // are found instead of demanding one window for the whole trek.
            let (dx, dz) = (goal_x - src_x, goal_z - src_z);
            // Fan of hop targets: the direct step first, then 16 bearings at
            // 40 tiles, most promising (closest to the destination) first.
            // A single window cannot solve a whole trek: bridges, gates and
            // castle walls force detours the fan discovers step by step.
            // No 80-tile ring: the BFS window only spans +-64 tiles, so
            // farther hops plan to window edges instead of real paths and
            // the walk swings instead of committing.
            let mut candidates: Vec<(i32, i32)> = Vec::new();
            let direct_x = dx.clamp(-55, 55);
            let direct_z = dz.clamp(-55, 55);
            if (direct_x, direct_z) != (0, 0) {
                candidates.push((direct_x, direct_z));
            }
            for radius in [40.0f64] {
                for step in 0..16 {
                    let angle = std::f64::consts::TAU * f64::from(step) / 16.0;
                    let hop_x = src_x + (radius * angle.cos()).round() as i32;
                    let hop_z = src_z + (radius * angle.sin()).round() as i32;
                    if (hop_x, hop_z) != (src_x, src_z) {
                        candidates.push((hop_x - src_x, hop_z - src_z));
                    }
                }
            }
            candidates.sort_by_key(|hop| Self::hop_rank(goal_x, goal_z, src_x, src_z, *hop, &visits));
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
                let reach = (goal_x - (src_x + chosen.0)).abs() + (goal_z - (src_z + chosen.1)).abs();
                if reach < best_reach {
                    best_reach = reach;
                    detours = 0;
                } else {
                    detours += 1;
                    if detours > 10 {
                        match self.bump_escape(entry, &mut bumped, &mut still) {
                            Err(err) => {
                                self.route.clear();
                                self.route_target = None;
                                return Err(err);
                            }
                            Ok(true) => continue,
                            Ok(false) => {}
                        }
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
                    // No plan at all: known doors already read open in the
                    // view, so a nearby click cannot create a route (the
                    // blockage is not a known door). Fail loud with the
                    // blockers instead of pacing speculative opens.
                    match self.bump_escape(entry, &mut bumped, &mut still) {
                        Err(err) => {
                            self.route.clear();
                            self.route_target = None;
                            return Err(err);
                        }
                        Ok(true) => continue,
                        Ok(false) => {}
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
            let final_hop = tiles.last() == Some(&(goal_x, goal_z));
            self.route = tiles.clone();
            let goal = tiles.last().copied();
            for chunk in tiles.chunks(STEP_CAP) {
                if self.world.ticks > deadline {
                    self.route.clear();
                    self.route_target = None;
                    return Err(Error::Timeout(format!("walk to {x},{z} timed out at {:?} ({} ticks)", self.position(), timeout)));
                }
                if let Err(err) = self.send_and_await(chunk, run, deadline) {
                    if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                        self.route.clear();
                        self.route_target = None;
                        return Err(err);
                    }
                    // Silent break replans blind: log what stopped the leg
                    // (server refusal, stall, timeout slice) with position.
                    self.log(&format!("leg stopped: {err} at {:?}", self.position()));
                    break;
                }
            }
            if self.arrived(goal_x, goal_z, level) {
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
                    eprintln!("{} walk stalled at {:?} (still={still})", self.tag(), self.position());
                }
                // Nothing next to us: the refused step may need a door
                // further along the route (adjacent ones open on the spot;
                // speculative nearby opens only fed the pacing).
                if doors_opened < 12 && self.open_door_on_path(&self.route.clone(), false) {
                    doors_opened += 1;
                    still = 0;
                    continue;
                }
                // Fully stalled with no near door helping: the first
                // route-door may sit ten tiles out, where whole-click
                // refusals stall and the near opener never fires. Walk to
                // it and open it there instead of pacing the same legs.
                if doors_opened < 12 && self.open_door_on_path(&self.route.clone(), true) {
                    doors_opened += 1;
                    still = 0;
                    continue;
                }
                still += 1;
            } else {
                still = 0;
            }
            if still >= 3 {
                // Legs sent, server never moved us, doors hold nothing:
                // last chance is the direct sidestep before the stall
                // error. A bump that moves replans once from there.
                match self.bump_escape(entry, &mut bumped, &mut still) {
                    Err(err) => {
                        self.route.clear();
                        self.route_target = None;
                        return Err(err);
                    }
                    Ok(true) => continue,
                    Ok(false) => {}
                }
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
    ///
    /// Tries the exact tile first, then a nearest-first fan of (2r+1)^2
    /// candidates. Two guards keep a stuck fan loud instead of pacing on
    /// dynamic doors: doors already opened and verified by this call fail
    /// fast on second encounter (see `approach_opened`), and the fan
    /// aborts after 10 candidates without getting closer to (x,z) with
    /// `no progress toward ...` (mirror of `walk_routed`'s `detours>10`).
    /// Passing walks behave tick for tick as before (no extra pump).

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{self, MapPack};

    /// Pack index with no squares: `ensure_area` loads nothing, so the
    /// hand-painted collision below survives untouched (offline stand-in
    /// for a loaded map window, no server needed).
    fn empty_pack() -> MapPack {
        MapPack::new(std::path::PathBuf::from("no-pack.zip"), std::collections::HashMap::new(), Vec::new())
    }

    /// Open 5x5 field with one blocked décor tile in the middle.
    fn field_with_blocked(bot: &mut Bot, dest_x: i32, dest_z: i32) {
        for tile_x in (dest_x - 3)..=(dest_x + 3) {
            for tile_z in (dest_z - 3)..=(dest_z + 3) {
                bot.collision.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        bot.collision.set(dest_x, dest_z, 0, collision::WALK_BLOCKED);
    }

    /// Blocked target, bot already on the adjacent tile: the router
    /// resolves the BFS-nearest tile (here) and returns Ok without ever
    /// sending a step — no pacing, no `walking in circles`.
    #[test]
    fn blocked_target_already_adjacent_returns_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 3216;
        bot.world.z = 3241;
        field_with_blocked(&mut bot, 3217, 3241);
        assert!(bot.walk_to(3217, 3241, 0, 600).is_ok());
        assert_eq!((bot.world.x, bot.world.z), (3216, 3241));
    }

    /// Walkable target, bot already there: Ok in both modes (straight legs
    /// without maps, routed with maps). The resolution only kicks in for
    /// blocked tiles, so this path is byte-for-byte the old one.
    #[test]
    fn exact_target_already_there_returns_ok() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 3215;
        bot.world.z = 3241;
        assert!(bot.walk_to(3215, 3241, 0, 600).is_ok());
        bot.maps = Some(empty_pack());
        bot.collision.set(3215, 3241, 0, collision::OPEN);
        assert!(bot.walk_to(3215, 3241, 0, 600).is_ok());
    }



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

    /// Bump trigger, pure: armed only on the entry tile and only before
    /// Hop rank prefers fresh tiles on ties: closeness first (routing
    /// unchanged), then fewest visits, so a walked tile loses the
    /// plan-A/plan-B tie instead of flip-flopping.
    #[test]
    fn hop_rank_prefers_unvisited_on_ties() {
        use std::collections::HashMap;
        let mut visits: HashMap<(i32, i32), u32> = HashMap::new();
        visits.insert((40, 0), 2);
        let back = Bot::hop_rank(40, 40, 0, 0, (40, 0), &visits);
        let fresh = Bot::hop_rank(40, 40, 0, 0, (0, 40), &visits);
        assert_eq!(back.0, fresh.0, "same closeness ties");
        assert!(fresh < back, "fresh tile wins the tie, got {fresh:?} vs {back:?}");
        let mut order = vec![(40, 0), (0, 40)];
        order.sort_by_key(|hop| Bot::hop_rank(40, 40, 0, 0, *hop, &visits));
        assert_eq!(order, vec![(0, 40), (40, 0)]);
    }

    /// Bump trigger, pure: armed only on the entry tile and only before
    /// the single bump+replan cycle fired. Moved walks and second
    /// give-ups never bump.
    #[test]
    fn bump_armed_only_on_entry_once() {
        let entry = (3095, 3240, 0);
        assert!(Bot::bump_armed(entry, (3095, 3240, 0), false), "give-up at entry arms");
        assert!(!Bot::bump_armed(entry, (3095, 3241, 0), false), "net movement disarms");
        assert!(!Bot::bump_armed(entry, (3095, 3240, 0), true), "second give-up disarms");
    }

    /// Offline every direct step is refused at the send (no socket, never
    /// logged in), so the bump surfaces the typed connection error instead
    /// of pacing dead sides: the session reconnects, never `Ok(false)`.
    #[test]
    fn bump_step_walled_in_returns_false_without_moving() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let start_ticks = bot.world.ticks;
        let err = bot.bump_step().expect_err("dead socket aborts typed");
        assert!(format!("{err}").contains("not logged in"), "typed conn error, got: {err}");
        assert_eq!((bot.world.x, bot.world.z), (0, 0), "bump never moved");
        assert_eq!(bot.world.ticks, start_ticks, "abort pumps nothing");
    }

    /// Open field, walkable target, but every send fails offline: the first
    /// chunk surfaces the typed connection error at once (reconnect), never
    /// the old `walking in circles` pacing — same text, no movement, no
    /// ticks, route cleared.
    #[test]
    fn circles_without_movement_keeps_original_error() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.maps = Some(empty_pack());
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        for tile_x in -3..=7 {
            for tile_z in -3..=3 {
                bot.collision.set(tile_x, tile_z, 0, collision::OPEN);
            }
        }
        let err = bot.walk_to(4, 0, 0, 600).unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("not logged in"), "typed conn error, got: {message}");
        assert_eq!((bot.world.x, bot.world.z), (0, 0), "no net movement");
        assert_eq!(bot.world.ticks, 0, "abort pumps nothing");
        assert!(bot.route.is_empty(), "route cleared");
        assert_eq!(bot.route_target, None, "target cleared");
    }

    /// Straight legs without maps surface the dead socket typed too: the
    /// first send fails, never a stall Timeout after the budget burns.
    #[test]
    fn straight_walk_offline_aborts_typed_not_timeout() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.level = 0;
        bot.world.x = 0;
        bot.world.z = 0;
        let err = bot.walk_to(4, 0, 0, 600).unwrap_err();
        assert!(format!("{err}").contains("not logged in"), "typed conn error, got: {err}");
        assert!(bot.route.is_empty(), "route cleared");
        assert_eq!(bot.route_target, None, "target cleared");
    }
}
