//! travel_to: one plan per trip, executed segment by segment.
//!
//! The plan is pure (same world, same options, same plan); this executor
//! only walks it and checks one postcondition per segment. A replanable
//! failure (step refused, shut door, dead or wrong ride) replans exactly
//! once with the faulty edge banned; the second failure goes to the
//! caller. Anything else (interrupt, io, timeout, choice dialogue) is
//! returned at once, never retried.
//!
//! Between-legs control (D65): one trip installs a `BetweenLegsHook`
//! (`install_between_legs`, always removed after) which the executor runs
//! between two sent legs — random event, injury and flight, death. The
//! hook observes only; the executor decides: `Continue` walks the next
//! leg, `Fled` replans from the new position with no edge banned, anything
//! else aborts the trip with a typed error (never a silent NoRoute).
//!
//! Plans go through the single-round-trip `plan` op (E3, server-side
//! wait, no 2 ms polling); walking costs go through one `walk_costs` op
//! per batch (N1, one search, order-preserving). No legacy submit/poll
//! path: every service failure is `nav-down`, never a local plan.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

use lifescape_json::Value;
use lifescape_nav::plan::{EdgeId, NavOptions, NoRoute, NoRouteReason, Plan, Segment, Tile};
use lifescape_nav::service::{self, Poll as ServicePoll};
#[cfg(test)]
use lifescape_nav::static_world::StaticWorld;
#[cfg(test)]
use lifescape_nav::tables::Tables;

use super::travel_dossier::{TRAIL_TAKE, dossier_json, trail_mark, write_dossier_file};
use super::travel_hook::{BetweenLegsHook, LegInfo, LegVerdict};
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
/// Toll dialog watch: the guard's lines stream in over several ticks
/// (ask, answer, then the 3-way choice), 25 covers the exchange plus a
/// slow-pump margin.
const TOLL_DIALOG_WAIT: u64 = 25;
/// Post-pick drain: one chatplayer page streams in after the choice and
/// the teleport follows the next continue, 10 covers the page plus a
/// slow-pump margin.
const TOLL_POST_PICK_WAIT: u64 = 10;
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
#[derive(Debug)]
pub(crate) struct PlanTimings {
    options_ms: u128,
    plan_ms: u128,
    service_ms: Option<u128>,
}

/// One remote plan attempt: ready, a real diagnosis, a service failure
/// (`nav-down`), or an operator stop.
#[derive(Debug)]
enum RemoteOutcome {
    Ready(Plan, u128),
    NoRoute(NoRoute),
    NavDown(String),
    Interrupted(String),
}

/// A terminal reply off a draining snapshot: ours is set and the served
/// one differs. Empty versions (tests, no daemon) never count as stale.
pub(crate) fn stale_reply(ours: &Option<String>, served: &str) -> bool {
    match ours {
        Some(ours) => !ours.is_empty() && !served.is_empty() && ours != served,
        None => false,
    }
}

/// Fled marker: the between-legs hook reports flight, the trip replans
/// from the new position with no edge banned (unlike a segment failure,
/// which bans the faulty edge). Prefix-matched, never shown to callers
/// beyond the dossier line.
pub(crate) const FLED_MARK: &str = "fled-between-legs";

/// Between-legs hook registry (D65): one trip installs its control, the
/// executor reads it between two sent legs, the trip removes it after.
/// Keyed by bot identity (never dereferenced): `Bot` keeps its frozen
/// shape, and one worker drives one bot at a time, so identity keys never
/// collide across sessions. The lock is only held for the lookup, never
/// across the hook call.
static LEG_HOOKS: OnceLock<Mutex<HashMap<usize, BetweenLegsHook>>> = OnceLock::new();

fn legs_registry() -> &'static Mutex<HashMap<usize, BetweenLegsHook>> {
    LEG_HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn legs_key(bot: &Bot) -> usize {
    bot as *const Bot as usize
}

/// Install the between-legs control for one trip: the executor calls it
/// between two sent legs until `remove_between_legs`. Walk scripts wrap
/// the trip with `with_between_legs` so no control leaks.
pub fn install_between_legs(bot: &mut Bot, hook: BetweenLegsHook) {
    legs_registry().lock().unwrap().insert(legs_key(bot), hook);
}

/// Remove the trip control (called after the trip on every path).
pub fn remove_between_legs(bot: &Bot) {
    legs_registry().lock().unwrap().remove(&legs_key(bot));
}

/// Run one trip with the between-legs control installed, removed
/// afterwards on every path.
pub fn with_between_legs<T>(bot: &mut Bot, hook: BetweenLegsHook, trip: impl FnOnce(&mut Bot) -> T) -> T {
    install_between_legs(bot, hook);
    let out = trip(bot);
    remove_between_legs(bot);
    out
}

impl Bot {
    /// Travel to a tile on any level: plan, publish the full plan as the
    /// live route, execute every segment in order. `NoRoute` becomes
    /// `Error::Travel` with the planner diagnosis.
    pub fn travel_to(&mut self, dest: Tile) -> Result<()> {
        let (options, plan, timings) = self.plan_travel(dest)?;
        let timeout = plan.cost as u64 * COST_PER_TICK + COST_BASE;
        self.run_with_replan(dest, options, plan, timings, timeout)
    }

    /// Same-level-or-not walk with a caller tick budget. Thin envelope
    /// over `travel_to`; the signature stays for existing callers.
    pub fn walk_to(&mut self, x: i32, z: i32, level: i32, timeout: u64) -> Result<()> {
        self.act(format!("walk_to ({x},{z},{level}) timeout={timeout}"));
        let dest = Tile { x, z, level };
        let (options, plan, timings) = self.plan_travel(dest)?;
        self.run_with_replan(dest, options, plan, timings, timeout)
    }

    /// Plan through the service only: no local world, no local tables, no
    /// local search. Options come from the served `wants` tags pinned by
    /// version; without them the failure is `nav-down`, never an empty
    /// guess. Every service failure (down, refused, stale after 1
    /// resubmit, timeout, interrupt) is `nav-down` — never a local plan.
    pub(crate) fn plan_travel(&mut self, dest: Tile) -> Result<(NavOptions, Plan, PlanTimings)> {
        let options_start = std::time::Instant::now();
        let options = self.nav_options_remote()?;
        let options_ms = options_start.elapsed().as_millis();
        let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        let plan_start = std::time::Instant::now();
        let (plan, service_ms) = match self.plan_remote(&options, from, dest) {
            RemoteOutcome::Ready(plan, ipc_ms) => (plan, Some(ipc_ms)),
            RemoteOutcome::NoRoute(no) => return Err(self.fail_no_route(from, dest, &options, no)),
            RemoteOutcome::Interrupted(message) => return Err(Error::Interrupted(message)),
            RemoteOutcome::NavDown(reason) => return Err(Error::Travel(format!("nav-down: {reason}"))),
        };
        // A zero-step plan adjusted on place while the destination is far
        // is a disconnected region the planner reports as Found (blocked
        // dest, closest is here): executing it is a no-op Ok the gather
        // loops read as arrived, then hop unreachable spots for the whole
        // stall budget. Fail loud as NoRoute instead (near no-ops — the
        // repeat-visit click spot — stay Ok).
        if plan.segments.is_empty()
            && plan.adjusted_dest == Some(from)
            && (from.x - dest.x).abs() + (from.z - dest.z).abs() > 4
        {
            return Err(self.fail_no_route(
                from,
                dest,
                &options,
                NoRoute {
                    closest_reachable: from,
                    blocked_by_requirements: Vec::new(),
                    disabled_transitions_nearby: Vec::new(),
                    reason: NoRouteReason::Disconnected,
                    blockers: Vec::new(),
                },
            ));
        }
        let plan_ms = plan_start.elapsed().as_millis();
        Ok((options, plan, PlanTimings { options_ms, plan_ms, service_ms }))
    }

    /// Remote plan through one `plan` op with a server-side wait (one
    /// connection per plan, no 2 ms polling). Every refusal (down,
    /// refused, stale twice, timeout, interrupt, unknown op) is
    /// `nav-down`, never a local plan. Stale version: the served version
    /// replaces ours and the `plan` retries exactly once; a second skew
    /// is `nav-down`, never a loop.
    fn plan_remote(&mut self, options: &NavOptions, from: Tile, dest: Tile) -> RemoteOutcome {
        let client = match self.plan_client.clone() {
            Some(client) => client,
            None => return RemoteOutcome::NavDown("no-service-handle".to_string()),
        };
        self.plan_once_terminal(&client, options, from, dest)
    }

    /// E3 single round-trip: `plan` with one stale-version resubmit.
    /// Long computes wait server-side in 500 ms slices: between slices
    /// the keepalive still ticks (no 100-tick reap) and the operator stop
    /// still aborts (no blind 8 s wait). Median plans cost 1 connection.
    fn plan_once_terminal(
        &mut self,
        client: &service::PlanClient,
        options: &NavOptions,
        from: Tile,
        dest: Tile,
    ) -> RemoteOutcome {
        use lifescape_nav::service_plan::{PlanOnceError, PLAN_SLICE_WAIT_MS};
        let started = std::time::Instant::now();
        let deadline = started + service::SERVICE_WAIT_MAX;
        // A stop already posed (unload/close) aborts before any socket
        // traffic: ranking and costing helpers keep their no-block
        // contract, the run maps this to `Interrupted` like before.
        if !self.plan_yield() {
            return RemoteOutcome::Interrupted(format!(
                "plan aborted at {:?}",
                self.position()
            ));
        }
        let mut resubmits = 0u32;
        loop {
            if std::time::Instant::now() >= deadline {
                return RemoteOutcome::NavDown("service-timeout".to_string());
            }
            let version = self.plan_data_version.clone().unwrap_or_default();
            match client.plan_once(from, dest, options, &version, PLAN_SLICE_WAIT_MS) {
                Ok((ServicePoll::Ready(plan), served)) => {
                    if stale_reply(&self.plan_data_version, &served) {
                        self.plan_data_version = Some(served.clone());
                        if resubmits >= 1 {
                            return RemoteOutcome::NavDown(format!(
                                "version-skew server={served}"
                            ));
                        }
                        resubmits += 1;
                        continue;
                    }
                    let ipc_ms = started.elapsed().as_millis();
                    return RemoteOutcome::Ready(plan, ipc_ms);
                }
                Ok((ServicePoll::Failed(no), served)) => {
                    if stale_reply(&self.plan_data_version, &served) {
                        self.plan_data_version = Some(served.clone());
                        if resubmits >= 1 {
                            return RemoteOutcome::NavDown(format!(
                                "version-skew server={served}"
                            ));
                        }
                        resubmits += 1;
                        continue;
                    }
                    return if no.reason == NoRouteReason::Aborted {
                        RemoteOutcome::NavDown("server-aborted".to_string())
                    } else {
                        RemoteOutcome::NoRoute(no)
                    };
                }
                Ok((ServicePoll::Pending, _)) | Ok((ServicePoll::Unknown, _)) => {
                    return RemoteOutcome::NavDown(
                        "plan-op-nonterminal".to_string(),
                    );
                }
                Err(PlanOnceError::Refused { reason, served }) => {
                    if reason == "plan-timeout" {
                        // Slice elapsed, plan still computing: keepalive +
                        // stop check, then wait another slice (250x fewer
                        // connections than the old 2 ms poll loop).
                        if !self.plan_yield() {
                            return RemoteOutcome::Interrupted(format!(
                                "plan aborted at {:?}",
                                self.position()
                            ));
                        }
                        continue;
                    }
                    if reason.starts_with("refused:stale-version") && resubmits < 1 && !served.is_empty() {
                        self.plan_data_version = Some(served);
                        resubmits += 1;
                        continue;
                    }
                    // No legacy submit/poll path (N9): an old daemon
                    // without the op fails loud as `nav-down`.
                    if reason.contains("unknown op") {
                        return RemoteOutcome::NavDown(reason);
                    }
                    return RemoteOutcome::NavDown(reason);
                }
                Err(PlanOnceError::Transport(error)) => {
                    return RemoteOutcome::NavDown(error);
                }
            }
        }
    }

    /// Shared NoRoute mapping: sync and remote diagnoses become the same
    /// dossier-carrying error (wrapper, not fork).
    fn fail_no_route(&mut self, from: Tile, dest: Tile, options: &NavOptions, no: NoRoute) -> Error {
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
        self.fail(from, dest, options, None, None, &expected, &Error::Travel(observed))
    }

    /// Run the installed between-legs control, if any. `Continue` walks the
    /// next leg; `Fled` replans from the new position (marker error, never a
    /// segment failure, never an edge ban); `RandomEvent`/`Dead` abort the
    /// trip with a typed error the caller maps (rescue, flee, re-gear) —
    /// never a silent NoRoute, never a replan.
    fn check_legs(&mut self, info: LegInfo) -> Result<()> {
    let hook = legs_registry().lock().unwrap().get(&legs_key(self)).cloned();
    let Some(hook) = hook else {
        return Ok(());
    };
    match hook(self, &info) {
        LegVerdict::Continue => Ok(()),
        LegVerdict::Fled => Err(Error::StepRefused(format!(
            "{FLED_MARK} at {:?} after leg {}/{}",
            self.position(),
            info.seg_index + 1,
            info.seg_total,
        ))),
        LegVerdict::RandomEvent => Err(Error::Unreachable(format!(
            "random event between legs at {:?} after leg {}/{}",
            self.position(),
            info.seg_index + 1,
            info.seg_total,
        ))),
        LegVerdict::Dead => Err(Error::Unreachable(format!(
            "dead between legs at {:?} after leg {}/{}",
            self.position(),
            info.seg_index + 1,
            info.seg_total,
        ))),
    }
}

/// Yield point for long plans: the plan runs on the only socket
    /// reader, so each yield emits the keepalive the server needs (it
    /// reaps after ~6 s of silence: 100 ticks at 60 ms). The session stop
    /// hook aborts the same way. False stops every pending search; the
    /// caller maps it to `Interrupted`, never a dossier. Both failures log
    /// their source: a silent abort here ends the run, so the run must
    /// say whether the socket or the operator stopped it.
    fn plan_yield(&mut self) -> bool {
        if let Err(err) = self.keepalive_if_due() {
            self.log(&format!("plan yield: no keepalive ({err}), aborting"));
            return false;
        }
        if let Some(hook) = self.on_pump.clone() {
            if !hook(self) {
                self.log("plan yield: stopped by operator hook, aborting");
                return false;
            }
        }
        true
    }

    /// Options from live state: toll wallet, carried items, quest points.
    /// Tags the data never requires (`quest:<name>`...) stay unsatisfied,
    /// so the planner reports them instead of walking into a refusal.
    #[cfg(test)]
    pub(crate) fn nav_options(&self, tables: &Tables) -> NavOptions {
        let carried = crate::client::aggregate(&self.world.inv);
        let coins = carried.get(&COINS_OBJ).copied().unwrap_or(0);
        let quest_points = self.world.varps.get(&QP_VARP).copied().unwrap_or(0);
        let mut wants: HashSet<&str> = HashSet::new();
        for entry in &tables.requirements {
            wants.extend(entry.requires.iter().map(String::as_str));
        }
        for edge in &tables.customs {
            wants.extend(edge.requires.iter().map(String::as_str));
        }
        NavOptions {
            allow_paid: self.allow_paid,
            f2p_only: true,
            satisfied: Self::satisfy_tags(wants, &carried, coins, quest_points),
            excluded_edges: HashSet::new(),
        }
    }

    /// Options from the served `wants` tags, pinned by version: the
    /// worker builds them from its live state with no local tables. The
    /// cache holds while `plan_data_version` holds; a skew refetch is one
    /// round-trip. No service (or a refused/skewed one) is `nav-down`,
    /// never an empty satisfied set: the plan must fail loud instead of
    /// reporting legs the wallet already covers.
    pub(crate) fn nav_options_remote(&mut self) -> Result<NavOptions> {
        let pinned = self.plan_data_version.clone().unwrap_or_default();
        let tags = match &self.wants_cache {
            Some((cached_version, tags)) if *cached_version == pinned => tags.clone(),
            _ => {
                let tags = self.wants()?;
                let served = self.plan_data_version.clone().unwrap_or_default();
                self.wants_cache = Some((served, tags.clone()));
                tags
            }
        };
        let carried = crate::client::aggregate(&self.world.inv);
        let coins = carried.get(&COINS_OBJ).copied().unwrap_or(0);
        let quest_points = self.world.varps.get(&QP_VARP).copied().unwrap_or(0);
        Ok(NavOptions {
            allow_paid: self.allow_paid,
            f2p_only: true,
            satisfied: Self::satisfy_tags(tags.iter().map(String::as_str), &carried, coins, quest_points),
            excluded_edges: HashSet::new(),
        })
    }

    /// Tags the live state covers: coins, quest points, carried items.
    /// Anything else stays unsatisfied so the planner reports it.
    fn satisfy_tags<'tag>(
        wants: impl IntoIterator<Item = &'tag str>,
        carried: &HashMap<u16, u32>,
        coins: u32,
        quest_points: i32,
    ) -> HashSet<String> {
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
        satisfied
    }

    /// Batch walking costs in ONE `walk_costs` round-trip (N1): the
    /// service runs a single Dijkstra from here, stopped when every target
    /// is reached. Order-preserving (`costs[i]` answers `dests[i]`), `None`
    /// per unplannable target. Identical dests share one entry (E14
    /// grouping); options come from the served `wants` tags like a plan.
    /// Any miss (no service, refused, stale twice, stop, timeout,
    /// transport) is `None` for every dest, so callers keep the Manhattan
    /// order. Never plans locally.
    pub fn walk_costs(&mut self, dests: &[Tile]) -> Vec<Option<u32>> {
        if dests.is_empty() {
            return Vec::new();
        }
        let Some(client) = self.plan_client.clone() else {
            return vec![None; dests.len()];
        };
        let options = match self.nav_options_remote() {
            Ok(options) => options,
            Err(_) => return vec![None; dests.len()],
        };
        // A stop already posed aborts before any socket traffic: costing
        // helpers keep their no-block contract, like plans.
        if !self.plan_yield() {
            return vec![None; dests.len()];
        }
        let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        // E14: one remote entry per unique tile in the batch.
        let mut unique: Vec<Tile> = Vec::with_capacity(dests.len());
        let mut slot_of: Vec<usize> = Vec::with_capacity(dests.len());
        for dest in dests {
            match unique.iter().position(|known| *known == *dest) {
                Some(slot) => {
                    slot_of.push(slot);
                    self.cost_stats.dedup_hits += 1;
                }
                None => {
                    slot_of.push(unique.len());
                    unique.push(*dest);
                }
            }
        }
        let costs = self
            .walk_costs_remote(&client, &options, from, &unique)
            .unwrap_or_else(|| vec![None; unique.len()]);
        self.cost_stats.remote_calls += 1;
        slot_of.iter().map(|slot| costs[*slot]).collect()
    }

    /// One `walk_costs` round-trip with one stale-version resubmit: the
    /// served version replaces ours and the batch retries exactly once; a
    /// second skew is `None`, never a loop. `None`: down, refused, timeout
    /// or stop — never a local Dijkstra.
    fn walk_costs_remote(
        &mut self,
        client: &service::PlanClient,
        options: &NavOptions,
        from: Tile,
        dests: &[Tile],
    ) -> Option<Vec<Option<u32>>> {
        let mut resubmits = 0u32;
        loop {
            let version = self.plan_data_version.clone().unwrap_or_default();
            match client.walk_costs(from, dests, options, &version) {
                Ok((costs, served)) => {
                    if stale_reply(&self.plan_data_version, &served) {
                        self.plan_data_version = Some(served.clone());
                        if resubmits >= 1 {
                            return None;
                        }
                        resubmits += 1;
                        continue;
                    }
                    return Some(costs);
                }
                Err((reason, served)) => {
                    if reason.starts_with("refused:stale-version") && resubmits < 1 && !served.is_empty() {
                        self.plan_data_version = Some(served);
                        resubmits += 1;
                        continue;
                    }
                    return None;
                }
            }
        }
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

    /// Run every segment in order, at most one replan: a replanable
    /// segment failure replans with the faulty edge banned, a hook `Fled`
    /// between legs replans from the new position with no ban. Returns
    /// the second failure (or the replan diagnosis) to the caller; never
    /// retries anything else.
    fn run_with_replan(
        &mut self,
        dest: Tile,
        mut options: NavOptions,
        plan: Plan,
        timings: PlanTimings,
        timeout: u64,
    ) -> Result<()> {
        let deadline = self.world.ticks + timeout;
        self.log(&format!(
            "nav timing options={}ms plan={}ms{}",
            timings.options_ms,
            timings.plan_ms,
            timings.service_ms.map(|ms| format!(" service={ms}ms")).unwrap_or_default(),
        ));
        self.publish_plan(&plan, dest);
        let start = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
        let segments = plan.segments.len().max(1);
        match self.run_plan(&plan, segments, deadline) {
            Ok(()) => {
                self.route.clear();
                self.route_target = None;
                Ok(())
            }
            Err((failed, err)) if is_replanable(&err) => {
                // Fled (hook marker): the bot moved, nothing failed — replan
                // from the new position with no edge banned.
                let fled = matches!(&err, Error::StepRefused(detail) if detail.starts_with(FLED_MARK));
                let edge = if fled {
                    None
                } else {
                    ban_for(&plan.segments[failed.min(plan.segments.len().saturating_sub(1))], self)
                };
                let expected = seg_summary(&plan.segments[failed.min(plan.segments.len().saturating_sub(1))]);
                let first = self.fail(start, dest, &options, Some(&plan), Some(failed), &expected, &err);
                self.log(&format!("nav fail seg {}/{segments} at {:?}: {err} (dossier: {})", failed + 1, self.position(), dossier_text(&first)));
                if let Some(edge) = edge {
                    options.excluded_edges.insert(edge);
                }
                let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
                let replan_start = std::time::Instant::now();
                // The replan goes through the service with the same ban,
                // never a local search: a second service failure is
                // `nav-down`, the original failure keeps its dossier.
                let retry = match self.plan_remote(&options, from, dest) {
                    RemoteOutcome::Ready(retry, _) => retry,
                    RemoteOutcome::NoRoute(no) if no.reason == NoRouteReason::Aborted => {
                        return Err(Error::Interrupted(format!("replan aborted at {:?}", self.position())));
                    }
                    RemoteOutcome::NoRoute(no) => {
                        self.log(&format!("nav replan: {} (original failure stands)", no.reason.as_str()));
                        return Err(first);
                    }
                    RemoteOutcome::Interrupted(message) => return Err(Error::Interrupted(message)),
                    RemoteOutcome::NavDown(reason) => {
                        return Err(Error::Travel(format!("nav-down: {reason}")));
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
                match self.run_plan(&retry, segments, deadline) {
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
    /// failing segment index rides along for the replan ban. The door
    /// registry is seeded from the plan on entry and cleared on exit:
    /// ephemeral per execution, never painted, never kept.
    fn run_plan(&mut self, plan: &Plan, total: usize, deadline: u64) -> std::result::Result<(), (usize, Error)> {
        self.seed_plan_doors(plan);
        for (index, seg) in plan.segments.iter().enumerate() {
            let started = self.world.ticks;
            let summary = seg_summary(seg);
            let outcome = match seg {
                Segment::Walk { tiles } => self.exec_walk(tiles, deadline),
                Segment::OpenDoor { door, stand, cross_to, .. } => self.exec_door(door, *stand, *cross_to, deadline),
                Segment::Ride { transition, stand, pre_open, expect, .. } => {
                    self.exec_ride(transition, *stand, pre_open.clone(), *expect, deadline)
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
                    self.clear_plan_doors();
                    return Err((index, err));
                }
            }
            // Between-legs control (D65): event, injury and death ride the
            // hook between two sent legs, never the destination. The next
            // index rides along (a `Fled` replans the remainder).
            if index + 1 < plan.segments.len() {
                let kind = match seg {
                    Segment::Walk { .. } => "walk",
                    Segment::OpenDoor { .. } => "door",
                    Segment::Ride { .. } => "ride",
                    Segment::Custom { .. } => "custom",
                };
                if let Err(err) = self.check_legs(LegInfo { seg_index: index, seg_total: plan.segments.len(), kind }) {
                    self.log(&format!("nav legs {kind} {}/{} -> {err}", index + 1, plan.segments.len()));
                    self.clear_plan_doors();
                    return Err((index + 1, err));
                }
            }
        }
        self.clear_plan_doors();
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
        let legs_total = pending.len().div_ceil(STEP_CAP);
        let mut leg_no = 0usize;
        while !pending.is_empty() {
            if self.world.ticks > deadline {
                return Err(Error::Timeout(format!("walk timed out at {:?}", self.position())));
            }
            let end = pending.len().min(STEP_CAP);
            let leg: Vec<(i32, i32)> = pending[..end].to_vec();
            leg_no += 1;
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
            // Between-legs control inside long walks too: the next send
            // waits for the hook verdict.
            if !pending.is_empty() {
                self.check_legs(LegInfo { seg_index: leg_no - 1, seg_total: legs_total, kind: "walk" })?;
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
    /// message, so the log names the wall or the re-closed door. Static
    /// flags come from one service batch; when the batch itself is down
    /// and nothing live explains the stall, the failure is `nav-down`.
    fn refused(&mut self, pending: &[(i32, i32)], cause: &Error) -> Error {
        let start = (self.world.x, self.world.z, self.world.level);
        if let Some(suspect) = self.suspect_tile(pending, start) {
            return Error::StepRefused(format!("{}; walk: {cause}; last game: {}", suspect, last_message(self)));
        }
        let live = pending.first().map(|(x, z)| self.world.loc_at(*x, *z, start.2));
        let tile = pending.first().map(|(x, z)| (*x, *z, start.2));
        let flags = match tile {
            None => None,
            Some(tile) => match self.tiles_flags(std::slice::from_ref(&tile)) {
                Ok(rows) => rows.into_iter().next().map(|row| row.flags),
                Err(error) => return Error::Travel(format!("nav-down: {error}")),
            },
        };
        Error::StepRefused(format!(
            "{}; walk: {cause}; last game: {}",
            Self::refused_line(pending, start, live.flatten(), flags),
            last_message(self),
        ))
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
        // Already open: the live loc left the closed id (swapped away or
        // cleared). Step through at once, no click. A refused step means
        // the reading is stale (missed re-close) or the step was
        // transient: drop the stale entry and fall through to the click
        // path below, which re-opens a shut door and otherwise clicks a
        // no-op before stepping again. One recovery only: a second
        // refusal fails loud through the normal path.
        if self.door_live_open(door) {
            if self.step_door_through(door, cross_to, deadline).is_ok() {
                return Ok(());
            }
            self.world.locs.remove(&(door.x, door.z, door.level));
        }
        let open_op = door.open_op.ok_or_else(|| {
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
        // The plan names toll gates: only then is a pay dialog expected.
        // A dialog on a free door is unexpected — never auto-pay it, the
        // swing watch below (then the blind step) decides instead.
        if door.toll && self.take_toll_option()? {
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


    /// Toll dialog after a paid-door click: the guard's lines stream in
    /// over several ticks (ask, answer, then the 3-way choice), so pump
    /// until the options show, continuing through NPC chat on the way
    /// (same discipline as the quest drain). Only then pick the
    /// affirmative ("Yes, ok.", like the boat's 30-coin fare): the old
    /// single synchronous read almost always saw no options yet and
    /// fell through to DoorDidNotOpen (Al-Kharid gate live).
    /// True when an option was picked (re-watch the swing); a dialog
    /// with no yes/pay option is DoorDidNotOpen.
    fn take_toll_option(&mut self) -> Result<bool> {
        let watched = self.world.ticks + TOLL_DIALOG_WAIT;
        loop {
            match self.toll_pick()? {
                Some(picked) => {
                    self.chat_choose(&picked)?;
                    self.drain_toll_pick()?;
                    return Ok(true);
                }
                None => {
                    if self.world.modal.is_some() && !self.allow_paid {
                        return Err(Error::DoorDidNotOpen(format!(
                            "toll dialog at {:?} with paid paths off; last game: {}",
                            self.position(),
                            last_message(self),
                        )));
                    }
                    if self.world.ticks >= watched {
                        return Ok(false);
                    }
                    // NPC chat lines need a continue before the choice
                    // appears; a trade modal is never ours to dismiss.
                    if self.world.modal.as_ref().is_some_and(|modal| modal.layer != "main") {
                        self.chat_continue()?;
                    }
                    self.pump(1)?;
                }
            }
        }
    }

    /// Post-pick drain: `~chatplayer("Yes, ok.")` needs one more
    /// continue before `inv_del` + teleport (`border_gate.rs2`,
    /// `chat.rs2` p_choice3/chatplayer). A plain chat page with no
    /// options means continue; a closed dialog or a new choice ends
    /// the drain and `finish_door` decides. Bounded like the pre-pick
    /// wait, so a stuck dialog fails loud instead of wedging the leg.
    fn drain_toll_pick(&mut self) -> Result<()> {
        let watched = self.world.ticks + TOLL_POST_PICK_WAIT;
        while self.world.ticks < watched {
            if !self.toll_continue_due() {
                return Ok(());
            }
            self.chat_continue()?;
            self.pump(1)?;
        }
        Ok(())
    }

    /// True while a plain chat page is up with no choice to pick: the
    /// pick landed and the server waits on the pause button. Pure so
    /// the post-pick drain is testable offline.
    fn toll_continue_due(&self) -> bool {
        self.world.modal.as_ref().is_some_and(|modal| modal.layer != "main") && self.chat_options().is_empty()
    }

    /// Pure pick over the CURRENT options (testable offline): the
    /// guard's affirmative, yes/pay-shaped. None while the dialog
    /// streams in.
    fn toll_pick(&self) -> Result<Option<String>> {
        let options = self.chat_options();
        if options.is_empty() {
            return Ok(None);
        }
        if !self.allow_paid {
            return Err(Error::DoorDidNotOpen(format!(
                "toll dialog at {:?} with paid paths off: {}; last game: {}",
                self.position(),
                options.iter().map(|(_, text)| text.clone()).collect::<Vec<_>>().join(" / "),
                last_message(self),
            )));
        }
        match options.iter().find(|(_, text)| {
            let lower = text.to_lowercase();
            lower.contains("yes") || lower.contains("pay") || lower == "ok."
        }) {
            Some((_, text)) => Ok(Some(text.clone())),
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
                // The plan carries the prober verdict: a known choice names
                // the no-recorded-pick suffix, an unknown one stays bare.
                let known = ride.needs_choice;
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
    use crate::waits::tick_ms;

    fn empty_plan() -> Plan {
        Plan { segments: Vec::new(), cost: 0, adjusted_dest: None, dest_blocker: None }
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
    fn no_service_handle_is_nav_down() {
        let mut bot = offline_bot(3222, 3222);
        // No handle at all: the plan fails loud, never local.
        let err = match bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }) {
            Ok(_) => panic!("no handle must not plan"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, Error::Travel(detail) if detail.contains("nav-down")),
            "missing handle is nav-down, got {err:?}",
        );
    }

    #[test]
    fn plan_needs_no_local_world_or_tables() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service = Service::start(Snapshot::new(world, tables, "test".to_string()), &test_socket("sdk-bare"), 1)
            .expect("service starts");
        // Bare bot: no local world, no local tables, only the handle.
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.x = 3228;
        bot.world.z = 3214;
        bot.world.level = 0;
        // Keepalive primed as just-sent: the poll loop never yields mid-plan.
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test".to_string());
        let (_, plan, _) = bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }).expect("bare bot plans remote");
        assert!(!plan.segments.is_empty(), "the door route lands with no local map");
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
        assert_eq!(value.get("tick_ms").as_i64(), Some(tick_ms() as i64));
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
    fn service_down_is_nav_down() {
        use lifescape_nav::service::{PlanClient, test_socket};
        let mut bot = offline_bot(3228, 3214);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        // No listener on this socket: the plan fails loud, never local.
        bot.plan_client = Some(PlanClient::new(&test_socket("absent"), "t01"));
        let err = match bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }) {
            Ok(_) => panic!("a down service must not plan"),
            Err(err) => err,
        };
        assert!(
            matches!(&err, Error::Travel(detail) if detail.contains("nav-down")),
            "down service is nav-down, got {err:?}",
        );
    }

    #[test]
    fn service_plan_matches_engine() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, encode_plan, test_socket};
        let (world, tables) = test_nav_context();
        let service =
            Service::start(Snapshot::new(world, tables, "test".to_string()), &test_socket("sdk-plan"), 1).expect("service starts");
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let mut bot = offline_bot(3228, 3214);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test".to_string());
        let (_, via, timings) = bot.plan_travel(dest).expect("service plans");
        assert!(timings.service_ms.is_some(), "service path timed");
        // Engine reference, same world/tables/options: the service wraps
        // plan(), never forks it.
        let (world, tables) = test_nav_context();
        let options = bot.nav_options(&tables);
        let sync = lifescape_nav::plan::plan(&world, &tables, Tile { x: 3228, z: 3214, level: 0 }, dest, &options)
            .expect("engine plans");
        assert_eq!(
            lifescape_json::stringify(&encode_plan(&via)),
            lifescape_json::stringify(&encode_plan(&sync)),
            "service wraps plan(), never forks",
        );
    }

    #[test]
    fn service_plan_honors_operator_stop() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service =
            Service::start(Snapshot::new(world, tables, "test".to_string()), &test_socket("sdk-stop"), 1).expect("service starts");
        let mut bot = offline_bot(3222, 3222);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test".to_string());
        // A stop already posed aborts before any socket traffic (the
        // 100-tick reap guard, by construction).
        bot.on_pump = Some(Arc::new(|_: &mut Bot| false));
        let err = match bot.plan_travel(Tile { x: 3250, z: 3420, level: 0 }) {
            Ok(_) => panic!("stop aborts the remote plan"),
            Err(err) => err,
        };
        assert!(matches!(err, Error::Interrupted(_)), "stop maps to Interrupted, got {err:?}");
        // The server survives the cancel: the next plan lands.
        bot.on_pump = None;
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.plan_travel(Tile { x: 3224, z: 3214, level: 0 }).expect("server healthy after cancel");
    }

    #[test]
    fn service_version_skew_resubmits_once() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, encode_plan, test_socket};
        let (world, tables) = test_nav_context();
        // Server on "served-v2", worker still on "stale-v1": the submit
        // refuses stale, the worker adopts the served version and retries
        // exactly once — the plan lands, no fallback, no loop.
        let service =
            Service::start(Snapshot::new(world, tables, "served-v2".to_string()), &test_socket("sdk-skew"), 1).expect("service starts");
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let mut bot = offline_bot(3228, 3214);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("served-v2".to_string());
        // Warmup on the aligned version (L2 + painted squares): the skew
        // below must exercise the resubmit path, not a cold compute past
        // the keepalive window.
        let _ = bot.plan_travel(dest).expect("warmup plans");
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.plan_data_version = Some("stale-v1".to_string());
        let (_, via, _) = bot.plan_travel(dest).expect("skew resubmits, never fails");
        assert!(!via.segments.is_empty(), "resubmitted plan lands");
        assert_eq!(bot.plan_data_version.as_deref(), Some("served-v2"), "worker adopts the served version");
        // Same version now: the next plan is a plain L1 hit, still remote.
        let (_, again, _) = bot.plan_travel(dest).expect("aligned version plans");
        assert_eq!(
            lifescape_json::stringify(&encode_plan(&via)),
            lifescape_json::stringify(&encode_plan(&again)),
            "same version, same plan"
        );
    }

    #[test]
    fn walk_costs_batch_serves_one_search() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service =
            Service::start(Snapshot::new(world, tables, "test-costs".to_string()), &test_socket("sdk-costs"), 1).expect("service starts");
        let mut bot = offline_bot(3230, 3241);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test-costs".to_string());
        // One duplicate in the batch: a single op serves all three.
        let dests = [
            Tile { x: 3252, z: 3237, level: 0 },
            Tile { x: 3203, z: 3246, level: 0 },
            Tile { x: 3203, z: 3246, level: 0 },
        ];
        let before = service.stats();
        let costs = bot.walk_costs(&dests);
        assert_eq!(costs.len(), 3, "one cost per dest, repeats included");
        assert_eq!(costs[1], costs[2], "repeats share one entry");
        assert!(costs.iter().all(|cost| cost.is_some()), "service costs all, got: {costs:?}");
        // Walk-cheap (3203,3246) beats the Manhattan lure, same verdict as
        // the unit plans, without ever planning thread-side.
        assert!(costs[1] < costs[0], "walk-cheap wins, got: {costs:?}");
        assert_eq!(bot.cost_stats(), (1, 1), "one remote call, one in-batch duplicate");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "no plan ticket consumed");
        assert_eq!(after.walk_costs - before.walk_costs, 1, "one batch counted");
        let again = bot.walk_costs(&dests);
        assert_eq!(again, costs, "repeat serves identical costs");
        assert_eq!(service.stats().walk_costs - before.walk_costs, 2, "each batch is one op");
    }

    #[test]
    fn walk_costs_without_service_is_manhattan() {
        let mut bot = offline_bot(3230, 3241);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        let dests = [Tile { x: 3252, z: 3237, level: 0 }, Tile { x: 3203, z: 3246, level: 0 }];
        assert_eq!(bot.walk_costs(&dests), vec![None, None], "no service keeps Manhattan order");
    }

    #[test]
    fn walk_costs_stop_cancels_without_blocking() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "test-costs-stop".to_string()),
            &test_socket("sdk-costs-stop"),
            1,
        )
        .expect("service starts");
        let mut bot = offline_bot(3230, 3241);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "t01"));
        bot.plan_data_version = Some("test-costs-stop".to_string());
        bot.on_pump = Some(Arc::new(|_: &mut Bot| false));
        let start = std::time::Instant::now();
        let dests = [Tile { x: 3252, z: 3237, level: 0 }, Tile { x: 3203, z: 3246, level: 0 }];
        assert_eq!(bot.walk_costs(&dests), vec![None, None], "stop keeps Manhattan order");
        assert!(start.elapsed() < service::SERVICE_WAIT_MAX, "stop cancels, never blocks");
    }

    #[test]
    fn legs_hook_registry_roundtrips() {
        let mut bot = offline_bot(3222, 3222);
        let info = LegInfo { seg_index: 0, seg_total: 2, kind: "walk" };
        // No hook installed: the check passes through.
        assert!(bot.check_legs(info).is_ok());
        install_between_legs(&mut bot, Arc::new(|_, _| LegVerdict::Continue));
        assert!(bot.check_legs(info).is_ok());
        remove_between_legs(&bot);
        assert!(bot.check_legs(info).is_ok(), "removed controls never leak into the next trip");
    }

    #[test]
    fn legs_hook_verdicts_map_to_typed_errors() {
        use crate::client::travel_hook::is_no_route;
        let mut bot = offline_bot(3222, 3222);
        let info = LegInfo { seg_index: 1, seg_total: 3, kind: "door" };
        install_between_legs(&mut bot, Arc::new(|_, _| LegVerdict::Fled));
        match bot.check_legs(info) {
            Err(Error::StepRefused(detail)) => assert!(detail.starts_with(FLED_MARK), "fled carries its marker, got: {detail}"),
            other => panic!("fled replans, never aborts, got: {other:?}"),
        }
        install_between_legs(&mut bot, Arc::new(|_, _| LegVerdict::RandomEvent));
        match bot.check_legs(info) {
            Err(Error::Unreachable(detail)) => assert!(!is_no_route(&Error::Unreachable(detail.clone())), "hook aborts are typed, never NoRoute"),
            other => panic!("event aborts the trip, got: {other:?}"),
        }
        install_between_legs(&mut bot, Arc::new(|_, _| LegVerdict::Dead));
        assert!(matches!(bot.check_legs(info), Err(Error::Unreachable(_))), "death aborts the trip");
        remove_between_legs(&bot);
        assert!(bot.check_legs(info).is_ok());
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

    /// Palace door (loc 1516, Open op): the plan carries the click, so the
    /// executor needs no pack — open_op rides on the segment.
    fn palace_door() -> lifescape_nav::plan::DoorRef {
        lifescape_nav::plan::DoorRef { loc: 1516, x: 3212, z: 3470, level: 0, open_op: Some(1), toll: false }
    }

    /// Stale-open palace door, refused first step (live t01/t02 loops):
    /// the stale entry is dropped and the click path runs instead of
    /// failing at once. Offline the click fails typed Login (dead
    /// socket aborts the recovery, never a door error): the Login
    /// proves the click left, the missing entry proves the stale read
    /// was dropped. Pre-fix this returns DoorDidNotOpen with the stale
    /// entry intact (the give-up the live loops hit).
    #[test]
    fn stale_open_door_recovers_with_click() {
        let mut bot = offline_bot(3212, 3471);
        bot.world.locs.insert((3212, 3470, 0), None);
        let door = palace_door();
        let err = bot
            .exec_door(&door, Tile { x: 3212, z: 3471, level: 0 }, Tile { x: 3212, z: 3470, level: 0 }, 1000)
            .unwrap_err();
        assert!(matches!(err, Error::Login(_)), "recovery clicks (typed conn error offline), got: {err:?}");
        assert!(bot.world.locs.get(&(3212, 3470, 0)).is_none(), "stale open entry dropped");
    }

    /// Shut palace door, no live entry: the closed path still clicks
    /// first (typed Login offline), and nothing is dropped from a live
    /// map that never claimed the door open.
    #[test]
    fn shut_door_still_clicks_first() {
        let mut bot = offline_bot(3212, 3471);
        bot.world.locs.insert((3212, 3470, 0), Some((1516, 0, 1)));
        let door = palace_door();
        let err = bot
            .exec_door(&door, Tile { x: 3212, z: 3471, level: 0 }, Tile { x: 3212, z: 3470, level: 0 }, 1000)
            .unwrap_err();
        assert!(matches!(err, Error::Login(_)), "shut door clicks (typed conn error offline), got: {err:?}");
        assert_eq!(
            bot.world.locs.get(&(3212, 3470, 0)),
            Some(&Some((1516, 0, 1))),
            "shut reading untouched"
        );
    }

    /// Toll pick over the CURRENT options only (offline, no pump): the
    /// guard's 3-way choice reads "Yes, ok.", empty options read None
    /// (the dialog streams in later), anything without yes/pay fails
    /// loud, and paid-off fails loud while options show.
    #[test]
    fn toll_pick_reads_the_guard_choice() {
        let mut bot = offline_bot(3267, 3227);
        assert_eq!(bot.toll_pick().expect("empty options never fail"), None, "streaming dialog picks nothing yet");
        bot.world.modal = Some(crate::world::Modal { layer: "chat".to_string(), interface_id: 2469 });
        bot.world.texts.insert(2471, "No thank you, I'll walk around.".to_string());
        bot.world.texts.insert(2472, "Who does my money go to?".to_string());
        bot.world.texts.insert(2473, "Yes, ok.".to_string());
        bot.allow_paid = true;
        assert_eq!(
            bot.toll_pick().expect("guard choice picks"),
            Some("Yes, ok.".to_string()),
            "affirmative, like the boat fare"
        );
        bot.allow_paid = false;
        assert!(
            matches!(bot.toll_pick(), Err(Error::DoorDidNotOpen(_))),
            "paid-off with options showing fails loud, never auto-pays"
        );
        bot.allow_paid = true;
        bot.world.texts.insert(2471, "No thank you, I'll walk around.".to_string());
        bot.world.texts.insert(2472, "Who does my money go to?".to_string());
        bot.world.texts.insert(2473, "The money goes to the city.".to_string());
        assert!(
            matches!(bot.toll_pick(), Err(Error::DoorDidNotOpen(_))),
            "no yes/pay option fails loud, got: {:?}",
            bot.toll_pick()
        );
    }

    /// Post-pick drain (`border_gate.rs2` + `chat.rs2`): after the
    /// "Yes, ok." pick the server shows a chatplayer page (`chat1`, com
    /// 2473 spent) and waits on the pause button before `inv_del` +
    /// teleport. The drain keeps a continue due while that plain page
    /// shows, drops it once a choice or a closed dialog lands, and the
    /// continue itself reaches the send path (dead socket reads
    /// `Login`, like the quest drains it mirrors).
    #[test]
    fn toll_post_pick_drains_the_chatplayer_page() {
        let mut bot = offline_bot(3267, 3227);
        assert!(!bot.toll_continue_due(), "no dialog, no continue");
        // The spent choice: chat1 page open, no options (com 2473 gone).
        bot.world.modal = Some(crate::world::Modal { layer: "chat".to_string(), interface_id: 968 });
        bot.world.texts.insert(2, "Yes, ok.".to_string());
        assert!(bot.toll_continue_due(), "plain chatplayer page needs the extra continue");
        assert!(
            matches!(bot.chat_continue(), Err(Error::Login(_))),
            "continue emitted (dead socket fails at login, never at the op name)"
        );
        // A fresh choice ends the drain instead of continuing past it.
        bot.world.modal = Some(crate::world::Modal { layer: "chat".to_string(), interface_id: 2469 });
        bot.world.texts.insert(2471, "No thank you, I'll walk around.".to_string());
        bot.world.texts.insert(2473, "Yes, ok.".to_string());
        assert!(!bot.toll_continue_due(), "choice page is picked, never continued");
        bot.world.modal = Some(crate::world::Modal { layer: "main".to_string(), interface_id: 1 });
        bot.world.texts.clear();
        assert!(!bot.toll_continue_due(), "main window is never continued");
        bot.world.modal = None;
        assert!(!bot.toll_continue_due(), "closed dialog ends the drain");
    }

    /// R9 island trap (b7e2ggbp): a zero-step plan adjusted on place
    /// while the destination is far is a disconnected region, never an
    /// arrival. The planner reports it as Found (blocked dest, closest
    /// is here); executing it is a no-op Ok the gather loops read as
    /// arrived, then hop Draynor<->Al Kharid for the whole stall budget
    /// (1055t, 3 fails, graph abandoned). Fail loud here instead so the
    /// spot bans and the run ends fast with a NoRoute dossier.
    #[test]
    fn far_zero_step_adjusted_plan_is_no_route() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "test-r9".to_string()),
            &test_socket("sdk-r9"),
            1,
        )
        .expect("service starts");
        let mut island = offline_bot(3132, 3133);
        island.last_keepalive = Some(std::time::Instant::now());
        island.last_send_tick = island.world.ticks;
        island.plan_client = Some(PlanClient::new(service.socket_path(), "r9"));
        island.plan_data_version = Some("test-r9".to_string());
        let water = Tile { x: 3086, z: 3227, level: 0 };
        let err = match island.plan_travel(water) {
            Ok(_) => panic!("island trap must not plan"),
            Err(err) => err,
        };
        let dossier = match &err {
            Error::TravelFailure(failure) => {
                assert_eq!(failure.kind, TravelErrorKind::NoRoute, "disconnected reads NoRoute, got {err:?}");
                failure.dossier.clone()
            }
            other => panic!("NoRoute dossier, never a bare error, got: {other:?}"),
        };
        if let Some(path) = dossier {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Healthy path unchanged: mainland bank to water still plans to the
    /// shore, and standing on the shore is already arrived (near no-op
    /// Ok, never NoRoute) so repeat visits keep clicking.
    #[test]
    fn mainland_water_still_plans_to_shore() {
        use lifescape_nav::service::{PlanClient, Service, Snapshot, test_socket};
        let (world, tables) = test_nav_context();
        let service = Service::start(
            Snapshot::new(world, tables, "test-r9b".to_string()),
            &test_socket("sdk-r9b"),
            1,
        )
        .expect("service starts");
        let mut mainland = offline_bot(3092, 3243);
        mainland.last_keepalive = Some(std::time::Instant::now());
        mainland.last_send_tick = mainland.world.ticks;
        mainland.plan_client = Some(PlanClient::new(service.socket_path(), "r9b"));
        mainland.plan_data_version = Some("test-r9b".to_string());
        let water = Tile { x: 3086, z: 3227, level: 0 };
        let (_, plan, _) = mainland.plan_travel(water).expect("mainland plans to water");
        assert!(!plan.segments.is_empty(), "shore plan walks, got {plan:?}");
        let mut shore = offline_bot(3087, 3228);
        shore.last_keepalive = Some(std::time::Instant::now());
        shore.last_send_tick = shore.world.ticks;
        shore.plan_client = Some(PlanClient::new(service.socket_path(), "r9b"));
        shore.plan_data_version = Some("test-r9b".to_string());
        let (_, settled, _) = shore.plan_travel(water).expect("shore repeat stays Ok");
        assert!(settled.segments.is_empty(), "already at the click spot, got {settled:?}");
    }
}
