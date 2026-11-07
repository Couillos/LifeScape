//! travel_to: plan once with lifescape-nav, execute segment by segment.
//!
//! The plan is pure (same world, same options, same plan); this executor
//! only walks it and checks one postcondition per segment. A replanable
//! failure (step refused, shut door, dead or wrong ride) replans exactly
//! once with the faulty edge banned; the second failure goes to the
//! caller. Anything else (interrupt, io, timeout, choice dialogue) is
//! returned at once, never retried.

use std::collections::HashSet;
use std::sync::Arc;

use lifescape_json::Value;
use lifescape_nav::plan::{self, EdgeId, NavOptions, NoRouteReason, Plan, Segment, Tile};
use lifescape_nav::static_world::StaticWorld;
use lifescape_nav::tables::Tables;

use super::travel_dossier::{TRAIL_TAKE, dossier_json, trail_mark, write_dossier_file};
use crate::client::Bot;
use crate::client::movement::STEP_CAP;
use crate::error::{Error, Result, TravelError, TravelErrorKind};

/// Coins obj id (`gamedata/data/objs.json`, name "coins").
const COINS_OBJ: u16 = 995;
/// Quest-point varp id (`gamedata/data/varps.json`, name "qp",
/// transmit=true, so the live value is always on hand).
const QP_VARP: u16 = 101;

/// Door swing watch: the click resolves through the interaction queue
/// (measured 17 ticks send-to-visible in a freshly streamed area), 23
/// leaves that plus a slow-pump margin.
const DOOR_SWING_WAIT: u64 = 23;
/// Ride landing watch: measured 2-9 ticks live across stairs, trapdoors
/// and dungeon ladders, 41 leaves two slow server round-trips.
const RIDE_WAIT: u64 = 41;
/// Trapdoor pre-open watch: the lid flips within a few ticks, 17 leaves
/// the swing plus two slow pumps.
const PRE_OPEN_WAIT: u64 = 17;
/// Crossing step for an open door: the tiles are adjacent, measured
/// 1-2 ticks, 7 leaves a re-closed fast door plus margin. Same bound
/// serves the single orthogonal sidestep before a click.
const CROSS_WAIT: u64 = 7;
/// Walk budget per planned cost unit: walking runs ~1 tile per 2 ticks,
/// 4 per unit plus 347 base leaves stalls and door queues inside.
const COST_PER_TICK: u64 = 4;
const COST_BASE: u64 = 347;

/// Per-phase wall-clock of one plan: a trivial leg must cost
/// milliseconds, so a seconds-long plan names its spinner here.
struct PlanTimings {
    options_ms: u128,
    plan_ms: u128,
}

impl Bot {
    /// Travel to a tile on any level: plan, publish the full plan as the
    /// live route, execute every segment in order. `NoRoute` becomes
    /// `Error::Travel` with the planner diagnosis.
    pub fn travel_to(&mut self, dest: Tile) -> Result<()> {
        let (world, tables, options, plan, timings) = self.plan_travel(dest)?;
        let timeout = plan.cost as u64 * COST_PER_TICK + COST_BASE;
        self.run_with_replan(dest, world, tables, options, plan, timings, timeout)
    }

    /// Same-level-or-not walk with a caller tick budget. Thin envelope
    /// over `travel_to`; the signature stays for existing callers.
    pub fn walk_to(&mut self, x: i32, z: i32, level: i32, timeout: u64) -> Result<()> {
        self.act(format!("walk_to ({x},{z},{level}) timeout={timeout}"));
        let dest = Tile { x, z, level };
        let (world, tables, options, plan, timings) = self.plan_travel(dest)?;
        self.run_with_replan(dest, world, tables, options, plan, timings, timeout)
    }

    fn plan_travel(&mut self, dest: Tile) -> Result<(Arc<StaticWorld>, Arc<Tables>, NavOptions, Plan, PlanTimings)> {
        let world = self.nav_world.clone().ok_or_else(|| {
            Error::State("travel_to without static world (the session wires it at open)".to_string())
        })?;
        let tables = self.nav_tables.clone().ok_or_else(|| {
            Error::State("travel_to without planner tables (the session wires them at open)".to_string())
        })?;
        let options_start = std::time::Instant::now();
        let options = self.nav_options(&tables);
        let options_ms = options_start.elapsed().as_millis();
        let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        let plan_start = std::time::Instant::now();
        let plan = plan::plan_yielding(&world, &tables, from, dest, &options, &mut || self.plan_yield()).map_err(|no| {
            if no.reason == NoRouteReason::Aborted {
                return Error::Interrupted(format!("plan aborted at {:?}", self.position()));
            }
            let mut blocked: Vec<String> =
                no.blocked_by_requirements.iter().map(|(edge, tags)| format!("{edge:?} missing {tags:?}")).collect();
            blocked.sort();
            let observed = format!(
                "no route ({},{},{}) -> ({},{},{}): {} closest=({},{},{}) blocked=[{}] disabled_nearby={}",
                from.x,
                from.z,
                from.level,
                dest.x,
                dest.z,
                dest.level,
                no.reason.as_str(),
                no.closest_reachable.x,
                no.closest_reachable.z,
                no.closest_reachable.level,
                blocked.join("; "),
                no.disabled_transitions_nearby.len(),
            );
            let expected = format!("route ({},{},{}) -> ({},{},{})", from.x, from.z, from.level, dest.x, dest.z, dest.level);
            self.fail(from, dest, &options, None, None, &expected, &Error::Travel(observed))
        })?;
        let plan_ms = plan_start.elapsed().as_millis();
        Ok((world, tables, options, plan, PlanTimings { options_ms, plan_ms }))
    }

    /// Yield point for long plans: the plan runs on the only socket
    /// reader, so each yield emits the keepalive the server needs (it
    /// reaps after ~6 s of silence: 100 ticks at 60 ms). The session stop
    /// hook aborts the same way. False stops every pending search; the
    /// caller maps it to `Interrupted`, never a dossier.
    fn plan_yield(&mut self) -> bool {
        if self.keepalive_if_due().is_err() {
            return false;
        }
        if let Some(hook) = self.on_pump.clone() {
            return hook(self);
        }
        true
    }

    /// Options from live state: toll wallet, carried items, quest points.
    /// Tags the data never requires (`quest:<name>`...) stay unsatisfied,
    /// so the planner reports them instead of walking into a refusal.
    pub(crate) fn nav_options(&self, tables: &Tables) -> NavOptions {        let carried = crate::client::aggregate(&self.world.inv);
        let coins = carried.get(&COINS_OBJ).copied().unwrap_or(0);
        let quest_points = self.world.varps.get(&QP_VARP).copied().unwrap_or(0);
        let mut wants: HashSet<&str> = HashSet::new();
        for entry in &tables.requirements {
            wants.extend(entry.requires.iter().map(String::as_str));
        }
        for edge in &tables.customs {
            wants.extend(edge.requires.iter().map(String::as_str));
        }
        let mut satisfied = HashSet::new();
        for tag in wants {
            if let Some(need) = tag.strip_prefix("coins:") {
                if coins >= need.parse::<u32>().unwrap_or(u32::MAX) {
                    satisfied.insert(tag.to_string());
                }
            } else if let Some(need) = tag.strip_prefix("qp:") {
                if quest_points >= need.parse::<i32>().unwrap_or(i32::MAX) {
                    satisfied.insert(tag.to_string());
                }
            } else if let Some(id) = tag.strip_prefix("item:") {
                if id.parse::<u16>().ok().is_some_and(|obj| carried.contains_key(&obj)) {
                    satisfied.insert(tag.to_string());
                }
            }
        }
        NavOptions { allow_paid: self.allow_paid, f2p_only: true, satisfied, excluded_edges: HashSet::new() }
    }

    /// Full plan into the live route: the dashboard trace is the plan.
    fn publish_plan(&mut self, plan: &Plan, dest: Tile) {
        let mut trace = Vec::new();
        for seg in &plan.segments {
            match seg {
                Segment::Walk { tiles } => trace.extend(tiles.iter().map(|tile| (tile.x, tile.z))),
                Segment::OpenDoor { cross_to, .. } => trace.push((cross_to.x, cross_to.z)),
                Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => trace.push((expect.x, expect.z)),
            }
        }
        self.route = trace;
        self.route_target = Some((dest.x, dest.z));
        self.log(&format!(
            "nav plan {} segments cost {} from ({},{},{}) to ({},{},{}){}",
            plan.segments.len(),
            plan.cost,
            self.world.x,
            self.world.z,
            self.world.level,
            dest.x,
            dest.z,
            dest.level,
            plan.adjusted_dest.map(|at| format!(" adjusted=({},{},{})", at.x, at.z, at.level)).unwrap_or_default(),
        ));
    }

    /// Run every segment in order, at most one replan on a replanable
    /// failure. Returns the second failure (or the replan diagnosis) to
    /// the caller; never retries anything else.
    fn run_with_replan(
        &mut self,
        dest: Tile,
        world: Arc<StaticWorld>,
        tables: Arc<Tables>,
        mut options: NavOptions,
        plan: Plan,
        timings: PlanTimings,
        timeout: u64,
    ) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        // Live state around the start: painted collision, door registry
        // and static locs, so suspect lines and door syncs see the world
        // instead of empty maps.
        let ensure_start = std::time::Instant::now();
        self.ensure_area(self.world.x, self.world.z, 70);
        let ensure_ms = ensure_start.elapsed().as_millis();
        self.log(&format!(
            "nav timing options={}ms plan={}ms ensure_area={}ms",
            timings.options_ms, timings.plan_ms, ensure_ms,
        ));
        self.publish_plan(&plan, dest);
        let start = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        let segments = plan.segments.len().max(1);
        match self.run_plan(&plan, &tables, segments, deadline) {
            Ok(()) => {
                self.route.clear();
                self.route_target = None;
                Ok(())
            }
            Err((failed, err)) if is_replanable(&err) => {
                let edge = ban_for(&plan.segments[failed.min(plan.segments.len().saturating_sub(1))], self);
                let expected = seg_summary(&plan.segments[failed.min(plan.segments.len().saturating_sub(1))]);
                let first = self.fail(start, dest, &options, Some(&plan), Some(failed), &expected, &err);
                self.log(&format!("nav fail seg {}/{segments} at {:?}: {err} (dossier: {})", failed + 1, self.position(), dossier_text(&first)));
                if let Some(edge) = edge {
                    options.excluded_edges.insert(edge);
                }
                let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
                let replan_start = std::time::Instant::now();
                let retry = match plan::plan_yielding(&world, &tables, from, dest, &options, &mut || self.plan_yield()) {
                    Ok(retry) => retry,
                    Err(no) if no.reason == NoRouteReason::Aborted => {
                        return Err(Error::Interrupted(format!("replan aborted at {:?}", self.position())));
                    }
                    Err(no) => {
                        self.log(&format!("nav replan: {} (original failure stands)", no.reason.as_str()));
                        return Err(first);
                    }
                };
                self.log(&format!(
                    "nav replan {} segments cost {} in {}ms",
                    retry.segments.len(),
                    retry.cost,
                    replan_start.elapsed().as_millis(),
                ));
                self.publish_plan(&retry, dest);
                let segments = retry.segments.len().max(1);
                match self.run_plan(&retry, &tables, segments, deadline) {
                    Ok(()) => {
                        self.route.clear();
                        self.route_target = None;
                        Ok(())
                    }
                    Err((second, second_err)) => {
                        let expected = seg_summary(&retry.segments[second.min(retry.segments.len().saturating_sub(1))]);
                        Err(self.fail(start, dest, &options, Some(&retry), Some(second), &expected, &second_err))
                    }
                }
            }
            Err((failed, err)) => {
                let expected = plan.segments.get(failed).map(seg_summary).unwrap_or_else(|| format!("route to ({},{},{})", dest.x, dest.z, dest.level));
                Err(self.fail(start, dest, &options, Some(&plan), Some(failed), &expected, &err))
            }
        }
    }

    /// One pass over a plan. Ok when every postcondition holds; the
    /// failing segment index rides along for the replan ban.
    fn run_plan(&mut self, plan: &Plan, tables: &Tables, total: usize, deadline: u64) -> std::result::Result<(), (usize, Error)> {
        for (index, seg) in plan.segments.iter().enumerate() {
            let started = self.world.ticks;
            let summary = seg_summary(seg);
            let outcome = match seg {
                Segment::Walk { tiles } => self.exec_walk(tiles, deadline),
                Segment::OpenDoor { door, stand, cross_to } => self.exec_door(door, *stand, *cross_to, deadline),
                Segment::Ride { transition, stand, pre_open, expect } => {
                    self.exec_ride(transition, *stand, pre_open.clone(), *expect, tables, deadline)
                }
                Segment::Custom { edge, .. } => {
                    Err(Error::Travel(format!("custom edge {} has no live executor", edge.name)))
                }
            };
            match outcome {
                Ok(()) => self.log(&format!(
                    "nav seg {}/{} {} -> ok en {} ticks",
                    index + 1,
                    total,
                    summary,
                    self.world.ticks.saturating_sub(started),
                )),
                Err(err) => {
                    self.log(&format!(
                        "nav seg {}/{} {} -> FAIL {err} en {} ticks",
                        index + 1,
                        total,
                        summary,
                        self.world.ticks.saturating_sub(started),
                    ));
                    return Err((index, err));
                }
            }
        }
        Ok(())
    }

    /// Walk legs of STEP_CAP steps; postcondition is the last tile. A
    /// stalled leg (no progress, budget left) is a refused step.
    fn exec_walk(&mut self, tiles: &[Tile], deadline: u64) -> Result<()> {
        if tiles.is_empty() {
            return Ok(());
        }
        let run = self.want_run();
        let mut pending: Vec<(i32, i32)> = tiles.iter().map(|tile| (tile.x, tile.z)).collect();
        while !pending.is_empty() {
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("walk timed out at {:?}", self.position())));
            }
            let end = pending.len().min(STEP_CAP);
            let leg: Vec<(i32, i32)> = pending[..end].to_vec();
            if let Err(err) = self.send_and_await(&leg, run, deadline, false) {
                if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                    return Err(err);
                }
                if self.world.ticks > deadline {
                    return Err(Error::Timeout(format!("walk timed out at {:?}", self.position())));
                }
                return Err(self.refused(&pending, &err));
            }
            let here = (self.world.x, self.world.z);
            match pending.iter().position(|tile| *tile == here) {
                Some(seen) => pending = pending.split_off(seen + 1),
                // Off the leg after a completed send: the plan no longer
                // describes where we are, so fail loud instead of dropping
                // the leg. StepRefused replans once from here (walks ban
                // nothing); a second miss returns this to the caller.
                None => {
                    return Err(Error::StepRefused(format!(
                        "walk left the leg at {:?}, want {pending:?}; last game: {}",
                        self.position(),
                        last_message(self),
                    )));
                }
            }
        }
        let last = tiles[tiles.len() - 1];
        if (self.world.x, self.world.z, self.world.level) != (last.x, last.z, last.level) {
            return Err(Error::StepRefused(format!(
                "walk ended at {:?}, want ({},{},{}); last game: {}",
                self.position(),
                last.x,
                last.z,
                last.level,
                last_message(self),
            )));
        }
        Ok(())
    }

    /// StepRefused for a walk stall: the suspect tile plus the last game
    /// message, so the log names the wall or the re-closed door.
    fn refused(&mut self, pending: &[(i32, i32)], cause: &Error) -> Error {
        let start = (self.world.x, self.world.z, self.world.level);
        let suspect = self
            .suspect_tile(pending, start)
            .unwrap_or_else(|| {
                let live = pending.first().map(|(x, z)| self.world.loc_at(*x, *z, start.2));
                let flags = pending.first().map(|(x, z)| self.collision.get(*x, *z, start.2)).unwrap_or(0);
                Self::refused_line(pending, start, live.flatten(), flags)
            });
        Error::StepRefused(format!("{}; walk: {cause}; last game: {}", suspect, last_message(self)))
    }

    /// Door segment, the single §6.2 model: read the live loc, step
    /// straight through when already open, else one open click, watch
    /// the live loc swing, then immediately step across (self-closing
    /// doors). Two measured extras, both bounded: wall_diagonal doors
    /// teleport the clicker onto the tile (content script
    /// `doors.rs2`, second click finishes), and a blind live stream
    /// can miss a real swing (one crossing attempt, server decides).
    /// Any failure is DoorDidNotOpen.
    fn exec_door(
        &mut self,
        door: &lifescape_nav::plan::DoorRef,
        stand: Tile,
        cross_to: Tile,
        deadline: u64,
    ) -> Result<()> {
        if (self.world.x, self.world.z, self.world.level) != (stand.x, stand.z, stand.level) {
            return Err(Error::StepRefused(format!(
                "door loc {}: off stand ({},{},{}) at {:?}",
                door.loc, stand.x, stand.z, stand.level, self.position(),
            )));
        }
        if self.world.ticks > deadline {
            return Err(Error::Timeout(format!("door loc {} timed out at {:?}", door.loc, self.position())));
        }
        // Live state around the door: painted square, so the open check
        // below reads the world instead of empty maps. The stand itself
        // is plan-guaranteed (orthogonal, engine reach-valid), never
        // adjusted here: a bad stand is a plan bug, reported loud.
        self.ensure_area(door.x, door.z, 3);
        // Already open: the live loc left the closed id (swapped away or
        // cleared). Step through at once, no click.
        if self.door_live_open(door) {
            return self.step_door_through(door, cross_to, deadline);
        }
        let open_op = self
            .maps
            .as_ref()
            .and_then(|pack| pack.loc_def(door.loc as usize))
            .and_then(|def| def.open_op)
            .ok_or_else(|| {
                Error::DoorDidNotOpen(format!(
                    "door loc {} at ({},{},{}) has no open op at {:?}; last game: {}",
                    door.loc, door.x, door.z, door.level, self.position(), last_message(self),
                ))
            })?;
        // One click only: the swing shows in the live loc within a few
        // ticks. A second click would toggle a swapped door shut again,
        // so never re-click for the walk-through model; the teleport
        // model moves us instead, which the watch below sees first.
        self.op_loc(open_op, door.x, door.z, door.loc)?;
        if self.take_toll_option()? {
            return self.finish_door(door, cross_to, deadline);
        }
        let watched = self.world.ticks + DOOR_SWING_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            if (self.world.x, self.world.z, self.world.level) == (cross_to.x, cross_to.z, cross_to.level) {
                return Ok(());
            }
            if (self.world.x, self.world.z) == (door.x, door.z) && self.world.level == door.level {
                // Teleported onto the tile: one more click goes through.
                return self.finish_teleport_door(door, open_op, cross_to, deadline);
            }
            if self.door_live_open(door) {
                // Swapped open: step through at once, before it re-closes.
                return self.step_door_through(door, cross_to, deadline);
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("door loc {} timed out at {:?}", door.loc, self.position())));
            }
        }
        // Blind swing: live locs do not always stream, so the watch can
        // miss a real opening. The steps are server-truth — an open door
        // lets them through, a shut one refuses — so try once, bounded.
        if self.step_door_through(door, cross_to, deadline).is_ok() {
            return Ok(());
        }
        Err(Error::DoorDidNotOpen(format!(
            "door loc {} at ({},{},{}) never swung from {:?}; last game: {}",
            door.loc, door.x, door.z, door.level, self.position(), last_message(self),
        )))
    }

    /// True when the live state no longer shows the closed id on the door
    /// tile: swapped to the open variant or explicitly cleared.
    fn door_live_open(&self, door: &lifescape_nav::plan::DoorRef) -> bool {
        match self.world.loc_at(door.x, door.z, door.level) {
            Some((id, _, _)) => id != door.loc,
            None => self.world.loc_removed(door.x, door.z, door.level),
        }
    }

    /// Walk the open door: one step onto the tile (unless on it), one off
    /// to cross_to, immediately. A refused step is DoorDidNotOpen.
    fn step_door_through(
        &mut self,
        door: &lifescape_nav::plan::DoorRef,
        cross_to: Tile,
        deadline: u64,
    ) -> Result<()> {
        let mut cross = Vec::new();
        if (self.world.x, self.world.z) != (door.x, door.z) {
            cross.push((door.x, door.z));
        }
        cross.push((cross_to.x, cross_to.z));
        let run = self.want_run();
        let cross_deadline = deadline.min(self.world.ticks + CROSS_WAIT);
        // Immediate send, exempt from the per-tick pacing: the swing watch
        // above already spent ticks, and this 1-2-tile MOVE races a
        // self-closing door (CROSS_WAIT is 7 ticks ≈ 420 ms; a 70 ms pacing
        // sleep would eat a sixth of that budget after the open is seen).
        // Arbitrage: a lone crossing packet cannot form a burst, while a
        // late step strands the bot behind a re-closed door — latency here
        // is worse than the FIN risk, so the exemption stands.
        if self.send_and_await(&cross, run, cross_deadline, true).is_err()
            || (self.world.x, self.world.z, self.world.level) != (cross_to.x, cross_to.z, cross_to.level)
        {
            return Err(Error::DoorDidNotOpen(format!(
                "door loc {} open but crossing to ({},{},{}) failed from {:?}; last game: {}",
                door.loc, cross_to.x, cross_to.z, cross_to.level, self.position(), last_message(self),
            )));
        }
        Ok(())
    }

    /// Teleport model, second half: parked on the door tile, click once
    /// more and land past it. No walking, no third click.
    fn finish_teleport_door(
        &mut self,
        door: &lifescape_nav::plan::DoorRef,
        open_op: u8,
        cross_to: Tile,
        deadline: u64,
    ) -> Result<()> {
        self.op_loc(open_op, door.x, door.z, door.loc)?;
        let watched = self.world.ticks + DOOR_SWING_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            if (self.world.x, self.world.z, self.world.level) == (cross_to.x, cross_to.z, cross_to.level) {
                return Ok(());
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("door loc {} timed out at {:?}", door.loc, self.position())));
            }
        }
        Err(Error::DoorDidNotOpen(format!(
            "door loc {} at ({},{},{}) left us at {:?}, want ({},{},{}); last game: {}",
            door.loc, door.x, door.z, door.level, self.position(), cross_to.x, cross_to.z, cross_to.level,
            last_message(self),
        )))
    }

    /// After a toll payment the swing is underway: land on cross_to by
    /// teleport or step, whichever the handler does.
    fn finish_door(
        &mut self,
        door: &lifescape_nav::plan::DoorRef,
        cross_to: Tile,
        deadline: u64,
    ) -> Result<()> {
        let watched = self.world.ticks + DOOR_SWING_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            if (self.world.x, self.world.z, self.world.level) == (cross_to.x, cross_to.z, cross_to.level) {
                return Ok(());
            }
            if self.door_live_open(door) {
                return self.step_door_through(door, cross_to, deadline);
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("door loc {} timed out at {:?}", door.loc, self.position())));
            }
        }
        Err(Error::DoorDidNotOpen(format!(
            "toll door loc {} at ({},{},{}) left us at {:?}; last game: {}",
            door.loc, door.x, door.z, door.level, self.position(), last_message(self),
        )))
    }


    /// Toll dialog after a paid-door click: pick the affirmative option so
    /// the swing proceeds. True when an option was picked (re-watch the
    /// swing); a dialog with no yes/pay option is DoorDidNotOpen.
    fn take_toll_option(&mut self) -> Result<bool> {
        let options = self.chat_options();
        if options.is_empty() {
            return Ok(false);
        }
        if !self.allow_paid {
            return Err(Error::DoorDidNotOpen(format!(
                "toll dialog at {:?} with paid paths off: {}; last game: {}",
                self.position(),
                options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / "),
                last_message(self),
            )));
        }
        let picked = options.iter().find(|(_, text)| {
            let lower = text.to_lowercase();
            lower.contains("yes") || lower.contains("pay") || lower == "ok."
        });
        match picked {
            Some((_, text)) => {
                let text = text.clone();
                self.chat_choose(&text)?;
                Ok(true)
            }
            None => Err(Error::DoorDidNotOpen(format!(
                "toll dialog at {:?} has no yes/pay option: {}; last game: {}",
                self.position(),
                options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / "),
                last_message(self),
            ))),
        }
    }

    /// Ride segment: stand on `stand`, pre-open trapdoors, click the
    /// climb op, land on `expect`. A choice dialogue is NeedsDialogueChoice
    /// (never auto-picked); stillness is RideDidNotMove, a wrong landing
    /// is RideWrongDestination.
    fn exec_ride(
        &mut self,
        ride: &lifescape_nav::plan::TransitionRef,
        stand: Tile,
        pre_open: Option<lifescape_nav::plan::OpenStep>,
        expect: Tile,
        tables: &Tables,
        deadline: u64,
    ) -> Result<()> {
        if (self.world.x, self.world.z, self.world.level) != (stand.x, stand.z, stand.level) {
            self.exec_walk(&[stand], deadline).map_err(|_| {
                Error::StepRefused(format!(
                    "ride {}: off stand ({},{},{}) at {:?}",
                    ride.name, stand.x, stand.z, stand.level, self.position(),
                ))
            })?;
        }
        if let Some(open) = pre_open {
            if self.world.loc_at(ride.from.x, ride.from.z, ride.from.level).map(|loc| loc.0) != Some(open.open_to) {
                self.op_loc(open.op, ride.from.x, ride.from.z, open.loc).map_err(|err| {
                    if crate::error::is_interrupted(&err) || matches!(err, Error::Io(_) | Error::Login(_)) {
                        err
                    } else {
                        Error::RideDidNotMove(format!("ride {}: pre-open click failed: {err}", ride.name))
                    }
                })?;
                let watched = self.world.ticks + PRE_OPEN_WAIT;
                let mut opened = false;
                while self.world.ticks < watched {
                    self.pump(1)?;
                    if self.world.loc_at(ride.from.x, ride.from.z, ride.from.level).map(|loc| loc.0)
                        == Some(open.open_to)
                    {
                        opened = true;
                        break;
                    }
                }
                if !opened {
                    return Err(Error::RideDidNotMove(format!(
                        "ride {}: lid loc {} never showed open state from {:?}; last game: {}",
                        ride.name, open.loc, self.position(), last_message(self),
                    )));
                }
            }
        }
        if self.world.ticks > deadline {
            return Err(Error::Timeout(format!("ride {} timed out at {:?}", ride.name, self.position())));
        }
        // Click the loc's answering tile: the footprint tile nearest
        // the stand (the table `from` is not always that tile on
        // multi-tile stairs). The engine may walk a step or two into
        // reach first, so only a real jump (level change or far tile)
        // ends the watch.
        let (click_x, click_z) = ride
            .footprint
            .iter()
            .min_by_key(|(tile_x, tile_z)| (tile_x - stand.x).abs() + (tile_z - stand.z).abs())
            .copied()
            .unwrap_or((ride.from.x, ride.from.z));
        self.op_loc(ride.op, click_x, click_z, ride.climb_loc)?;
        let start = (self.world.x, self.world.z, self.world.level);
        let watched = self.world.ticks + RIDE_WAIT;
        while self.world.ticks < watched {
            self.pump(1)?;
            let options = self.chat_options();
            if !options.is_empty() {
                let known = tables.verdict(ride.loc, ride.from).is_some_and(|seen| seen == lifescape_nav::tables::Verdict::NeedsChoice);
                let prompt = options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / ");
                return Err(Error::NeedsDialogueChoice(format!(
                    "ride {} opened a choice at {:?}: {prompt}{}; last game: {}",
                    ride.name,
                    self.position(),
                    if known { " (prober saw needs_choice, no recorded pick)" } else { "" },
                    last_message(self),
                )));
            }
            if (self.world.x, self.world.z, self.world.level) == (expect.x, expect.z, expect.level) {
                return Ok(());
            }
            let jumped = self.world.level != start.2
                || (self.world.x - start.0).abs().max((self.world.z - start.1).abs()) > 2;
            if jumped {
                return Err(Error::RideWrongDestination(format!(
                    "ride {} landed at {:?}, want ({},{},{}); last game: {}",
                    ride.name, self.position(), expect.x, expect.z, expect.level, last_message(self),
                )));
            }
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("ride {} timed out at {:?}", ride.name, self.position())));
            }
        }
        if (self.world.x, self.world.z, self.world.level) == start {
            return Err(Error::RideDidNotMove(format!(
                "ride {} never left {:?} in {RIDE_WAIT} ticks; last game: {}",
                ride.name,
                self.position(),
                last_message(self),
            )));
        }
        Err(Error::RideWrongDestination(format!(
            "ride {} drifted to {:?}, want ({},{},{}); last game: {}",
            ride.name, self.position(), expect.x, expect.z, expect.level, last_message(self),
        )))
    }
}

/// One-line plan summary for the `nav seg` log and `debug_walk`.
pub(crate) fn seg_summary(seg: &Segment) -> String {
    match seg {
        Segment::Walk { tiles } => {
            let (first, last) = (&tiles[0], &tiles[tiles.len() - 1]);
            format!(
                "Walk tiles={} ({},{},{})->({},{},{})",
                tiles.len(), first.x, first.z, first.level, last.x, last.z, last.level,
            )
        }
        Segment::OpenDoor { door, stand, cross_to } => format!(
            "OpenDoor loc={} stand=({},{},{}) cross=({},{},{})",
            door.loc, stand.x, stand.z, stand.level, cross_to.x, cross_to.z, cross_to.level,
        ),
        Segment::Ride { transition, stand, expect, .. } => format!(
            "Ride {} loc={} op={} stand=({},{},{}) expect=({},{},{})",
            transition.name,
            transition.climb_loc,
            transition.op,
            stand.x,
            stand.z,
            stand.level,
            expect.x,
            expect.z,
            expect.level,
        ),
        Segment::Custom { edge, stand, expect } => format!(
            "Custom {} stand=({},{},{}) expect=({},{},{})",
            edge.name, stand.x, stand.z, stand.level, expect.x, expect.z, expect.level,
        ),
    }
}

/// Only these four replan once with the faulty edge banned; everything
/// else (interrupt, io, timeout, choice, travel) goes to the caller.
fn is_replanable(err: &Error) -> bool {
    matches!(
        err,
        Error::StepRefused(_) | Error::DoorDidNotOpen(_) | Error::RideDidNotMove(_) | Error::RideWrongDestination(_)
    )
}

/// The edge to ban for one replan: the failing segment's own edge.
/// A walk leg is many steps, not one edge, so it bans nothing: the
/// single replan from the new position is the retry, and a nearby live
/// door stays unbanned until a plan names it (banning it blamed the
/// wrong edge when a stall came from a re-closed door elsewhere).
fn ban_for(seg: &Segment, _bot: &Bot) -> Option<EdgeId> {
    match seg {
        Segment::OpenDoor { door, .. } => {
            Some(EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level })
        }
        Segment::Ride { transition, .. } => Some(EdgeId::Ride(transition.name.clone())),
        Segment::Custom { edge, .. } => Some(EdgeId::Custom(edge.name.clone())),
        Segment::Walk { .. } => None,
    }
}

/// Last server game message, for failure context.
fn last_message(bot: &Bot) -> String {
    bot.world.messages.last().cloned().unwrap_or_else(|| "no game message".to_string())
}

#[cfg(test)]
pub(crate) fn test_nav_context() -> (Arc<StaticWorld>, Arc<Tables>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
    let data = root.join("LifeScape/gamedata/data");
    let gametypes = std::fs::read_to_string(data.join("gametypes.json")).expect("gametypes");
    let overrides = std::fs::read_to_string(data.join("nav_overrides.json")).expect("overrides");
    let defs = lifescape_nav::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
    let world = StaticWorld::open(&zip, defs).expect("static world");
    let zones = lifescape_nav::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
    world.set_f2p(zones);
    let tables = Tables::load(&data).expect("tables");
    world.set_sky_floors(tables.sky_floors.clone());
    (world, Arc::new(tables))
}


impl Bot {
    /// Build the §6.4 failure, write its dossier, log the path, and hand
    /// the wrapped error to the caller. A failed write never fails the
    /// caller: the error still carries kind, segment and positions.
    /// Transport and operator stops (Io, Login, Interrupted) keep their
    /// exact type: the session reconnect/stop arms match on them, and a
    /// dossier wrapper would hide the reconnect signal. Their dossier is
    /// still written and logged.
    fn fail(
        &self,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
        plan: Option<&Plan>,
        failed_segment: Option<usize>,
        expected: &str,
        inner: &Error,
    ) -> Error {
        let kind = TravelErrorKind::of(inner);
        let observed = inner.to_string();
        let position = (self.world.x, self.world.z, self.world.level);
        let tick = self.world.ticks;
        let skip = self.trail.len().saturating_sub(TRAIL_TAKE);
        let trail: Vec<Value> = self.trail[skip..].iter().map(trail_mark).collect();
        let value = dossier_json(
            &self.username,
            tick,
            from,
            dest,
            options,
            plan,
            failed_segment,
            kind,
            expected,
            &observed,
            &trail,
            &self.live_locs(),
            &self.static_live_diff(),
            &self.recent_messages(),
        );
        let dossier = write_dossier_file(&self.username, tick, &value);
        match &dossier {
            Some(path) => self.log(&format!("nav dossier {}", path.display())),
            None => self.log("nav dossier unwritten (io error), error stands"),
        }
        if matches!(kind, TravelErrorKind::Io | TravelErrorKind::Interrupted) {
            return inner.clone();
        }
        Error::TravelFailure(TravelError {
            kind,
            segment_index: failed_segment,
            position,
            tick,
            expected: expected.to_string(),
            observed,
            dossier,
        })
    }
}


/// One-line dossier pointer for the `nav fail` log.
fn dossier_text(error: &Error) -> String {
    match error {
        Error::TravelFailure(failure) => {
            failure.dossier.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| "unwritten".to_string())
        }
        _ => "unwritten".to_string(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::travel_dossier::dossier_json;
    use crate::waits::TICK_MS;

    fn empty_plan() -> Plan {
        Plan { segments: Vec::new(), cost: 0, adjusted_dest: None }
    }

    fn offline_bot(x: i32, z: i32) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        let (world, tables) = test_nav_context();
        bot.nav_world = Some(world);
        bot.nav_tables = Some(tables);
        bot.world.x = x;
        bot.world.z = z;
        bot.world.level = 0;
        bot
    }

    #[test]
    fn aborted_plan_is_interrupted() {
        let mut bot = offline_bot(3222, 3222);
        // No socket: the first yield's keepalive fails, aborting the search.
        // Interrupted here (not Travel/Disconnected) proves the yield fired
        // mid-search: without it this dest runs 238k visits to Disconnected.
        let err = match bot.plan_travel(Tile { x: 3100, z: 3100, level: 0 }) {
            Ok(_) => panic!("a refused yield must abort, not plan"),
            Err(err) => err,
        };
        assert!(matches!(err, Error::Interrupted(_)), "abort maps to Interrupted, got {err:?}");
    }

    #[test]
    fn short_plan_stays_offline() {
        let mut bot = offline_bot(3228, 3214);
        // No socket: pretend a keepalive just went out, so the setup
        // yield finds nothing due and the pure plan runs offline.
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        // Pure planning, no yield point reached: no socket, no keepalive, Ok.
        let (_, _, _, plan, _) = bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }).expect("short plan stays offline");
        assert!(!plan.segments.is_empty(), "the door route has segments");
    }

    #[test]
    fn dossier_carries_every_replay_input() {
        let from = Tile { x: 1, z: 2, level: 0 };
        let dest = Tile { x: 3, z: 4, level: 0 };
        let mut options = NavOptions::default();
        options.satisfied.insert("coins:10".to_string());
        let plan = empty_plan();
        let value = dossier_json(
            "t01",
            42,
            from,
            dest,
            &options,
            Some(&plan),
            Some(0),
            TravelErrorKind::StepRefused,
            "Walk 3 tiles",
            "step refused",
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(value.get("account").as_str(), Some("t01"));
        assert_eq!(value.get("tick").as_i64(), Some(42));
        assert_eq!(value.get("tick_ms").as_i64(), Some(TICK_MS as i64));
        assert_eq!(value.get("from").get("x").as_i64(), Some(1));
        assert_eq!(value.get("dest").get("z").as_i64(), Some(4));
        assert_eq!(value.get("failed_segment").as_i64(), Some(0));
        assert_eq!(value.get("error").get("kind").as_str(), Some("StepRefused"));
        assert_eq!(value.get("plan").get("cost").as_i64(), Some(0));
        for key in ["options", "trail", "live_locs", "static_live_diff", "messages"] {
            assert!(!value.get(key).is_null(), "dossier needs {key}");
        }
        assert_eq!(value.get("options").get("satisfied").as_arr().map(Vec::len), Some(1));
    }

    #[test]
    fn dossier_without_plan_marks_no_route() {
        let value = dossier_json(
            "t01",
            7,
            Tile { x: 0, z: 0, level: 0 },
            Tile { x: 9, z: 9, level: 0 },
            &NavOptions::default(),
            None,
            None,
            TravelErrorKind::NoRoute,
            "route",
            "no route",
            &[],
            &[],
            &[],
            &[],
        );
        assert!(value.get("plan").is_null());
        assert!(value.get("failed_segment").is_null());
        assert_eq!(value.get("error").get("kind").as_str(), Some("NoRoute"));
    }

    #[test]
    fn error_kind_covers_every_executor_outcome() {
        assert_eq!(TravelErrorKind::of(&Error::StepRefused("x".to_string())), TravelErrorKind::StepRefused);
        assert_eq!(TravelErrorKind::of(&Error::DoorDidNotOpen("x".to_string())), TravelErrorKind::DoorDidNotOpen);
        assert_eq!(TravelErrorKind::of(&Error::RideDidNotMove("x".to_string())), TravelErrorKind::RideDidNotMove);
        assert_eq!(
            TravelErrorKind::of(&Error::RideWrongDestination("x".to_string())),
            TravelErrorKind::RideWrongDestination
        );
        assert_eq!(
            TravelErrorKind::of(&Error::NeedsDialogueChoice("x".to_string())),
            TravelErrorKind::NeedsDialogueChoice
        );
        assert_eq!(TravelErrorKind::of(&Error::Timeout("x".to_string())), TravelErrorKind::Timeout);
        assert_eq!(TravelErrorKind::of(&Error::Interrupted("x".to_string())), TravelErrorKind::Interrupted);
        assert_eq!(TravelErrorKind::of(&Error::Io("x".to_string())), TravelErrorKind::Io);
        assert_eq!(TravelErrorKind::of(&Error::Travel("x".to_string())), TravelErrorKind::NoRoute);
    }
}
