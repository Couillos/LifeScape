#![forbid(unsafe_code)]
//! Market waits never idle the worker (user rule 2026-11-13: "aucun bot
//! inactif"; live: 12 bots of 25 idle on `awaiting the fill`).
//!
//! A `market:buy` placed daemon-side PARKS its node (`lifecycle::
//! park_for_fill`): ready, method kept, no lock, history `awaiting_fill`.
//! The bid rests on the book (the convene loop delivers a fill at the
//! worker's safe points, whatever it runs) while the launch pass:
//! - settles every parked wait each pass (`settle_parked`, the same rules
//!   as before: filled → done, partial → re-resolve, deadline → failure);
//! - launches the graph's next ready node that is not a parked wait
//!   (`next_ready_except`);
//! - with none, runs a FILLER: one short money window (`FILLER_TICKS`)
//!   chosen like a money node (plan équilibre step 3, `pick_window`: the
//!   money guide's lines on the shared demand, `money_pick`'s draw, only
//!   the lines a window works on its own; live 2026-11-19: a fixed list
//!   made fillers 20 % of the fleet's time, 52 % of it yew logs resting
//!   at 1 coin), steered off banned skills. Its node (`fill-<id>`) is not in
//!   the graph: the fold hands its report here (`fold_filler_report`),
//!   nothing is counted on the graph, but a filler that failed or ended
//!   without working (live 2026-11-14: `melee men` from the mill top,
//!   goal_reached after 0 ticks, relaunched every 10 s for an hour) keeps
//!   its skill out of the next fillers for `FILLER_RETRY_TICKS`.
//!   Its product goes on the public order board for the window (plan
//!   équilibre step 2, `brain_money::claim_launch`), released at its
//!   report.

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::expand::ReadyCtx;
use lifescape_planner::guide_pick::GuideLine;
use lifescape_planner::{Life, Node, NodeKind, NodeStatus};

use crate::brain::push_history;
use crate::fleet::Fleet;
use crate::jobs::Job;

/// Ticks of one filler window (then the pass rechecks the waits).
pub const FILLER_TICKS: u64 = 1500;
/// A failed or empty filler keeps its skill out of the fillers this long.
pub const FILLER_RETRY_TICKS: u64 = 500;
/// A refused filler attempt (no work, blocked, no script) is tried again
/// after this many ticks, never every pass.
pub const FILLER_REFUSAL_TICKS: u64 = 50;
/// History prefix of a filler launch (`filler:<method>`).
pub const FILLER_LAUNCH_TAG: &str = "filler:";
/// History prefix of a filler that failed (`filler_failed:<method>`).
const FILLER_FAILED_TAG: &str = "filler_failed:";

/// True for a filler node id (`fill-<parked id>`).
pub fn is_filler_node(node_id: &str) -> bool {
    node_id.starts_with("fill-")
}

/// What a filler report says: `Some(true)` it worked (ticks spent, no
/// error, ended on its goal or budget), `Some(false)` it failed or ended
/// at once without working, `None` an operator stop (neutral).
pub(crate) fn filler_worked(report: &Value) -> Option<bool> {
    let stop = report.get("stop").as_str().unwrap_or("");
    if stop == "stopped" {
        return None;
    }
    let ticks = report.get("ticks").as_i64().unwrap_or(0);
    let errored = report.get("error").get("type").as_str().is_some();
    Some(matches!(stop, "goal_reached" | "budget_exhausted") && ticks > 0 && !errored)
}

/// Fold one filler report into the history (never into the graph).
/// Returns the log line.
pub(crate) fn fold_filler_report(life: &mut Life, node_id: &str, report: &Value) -> String {
    // The window is over: its units are no longer on their way (plan
    // équilibre step 2), whatever it made.
    crate::market_claims::release_run(&life.account, node_id);
    // The launch tag names the method at its site (`method@x,z,level`).
    let launched = life
        .history
        .iter()
        .rev()
        .filter(|entry| entry.node == node_id)
        .find_map(|entry| entry.outcome.strip_prefix(FILLER_LAUNCH_TAG))
        .unwrap_or("?")
        .to_string();
    let (method, site) = lifescape_planner::cooldown::split_key(&launched);
    let method = method.to_string();
    let stop = report.get("stop").as_str().unwrap_or("?").to_string();
    let ticks = report.get("ticks").as_i64().unwrap_or(0);
    let error = report.get("error").get("message").as_str().unwrap_or("").to_string();
    match filler_worked(report) {
        Some(true) => {
            push_history(life, node_id, "filler_done");
            format!("filler {node_id} ({method}) worked: {stop} after {ticks} ticks")
        }
        Some(false) => {
            push_history(life, node_id, &format!("{FILLER_FAILED_TAG}{method}"));
            // Same cooldown as a node's abandon (tour 2 P2-6): the method
            // at its site never comes back hot, filler or node (live:
            // bellarhodes, 34 woodcut fillers in 10 min, "missing
            // bronze_axe" every time).
            let cause = if error.is_empty() { format!("filler {stop} after {ticks} ticks") } else { error.clone() };
            // A contended site cools like a contended node's (D4): the
            // site only, longer each time (live 2026-11-20: the same
            // filler sent back to the Wilderness big bones every ~8 min).
            let missing: Vec<String> = report.get("missing").as_arr().map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let contended = crate::brain_cool::contended_site(&missing);
            let depleted = crate::brain_cool::depleted_site(&missing);
            match (contended, depleted) {
                _ if method == "?" => {}
                // Every node respawning, another site sooner (user
                // 2026-11-20): the site cools for the respawn only.
                (_, Some((depleted, ticks))) => crate::brain_cool::on_depleted(life, &method, site.unwrap_or(depleted), ticks),
                (Some(contended), None) => crate::brain_cool::on_contended(life, &method, site.unwrap_or(contended)),
                (None, None) => {
                    crate::brain_cool::on_abandon(life, &method, site, &cause);
                }
            }
            format!("filler {node_id} ({method}) failed: {stop} after {ticks} ticks {error}; cooled, its skill sits out the fillers for {FILLER_RETRY_TICKS} ticks")
        }
        None => format!("filler {node_id} ({method}) stopped by the operator"),
    }
}

/// Skills of the fillers that failed within `FILLER_RETRY_TICKS`
/// (`ticks_since` maps a history stamp to elapsed ticks).
pub(crate) fn failed_filler_skills(life: &Life, ticks_since: &dyn Fn(i64) -> u64) -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();
    for entry in life.history.iter().filter(|entry| is_filler_node(&entry.node)) {
        let Some(method) = entry.outcome.strip_prefix(FILLER_FAILED_TAG) else { continue };
        if ticks_since(entry.ended) >= FILLER_RETRY_TICKS {
            continue;
        }
        if let Some(skill) = lifescape_planner::ambition_catalog::method_skill(method) {
            if !skills.iter().any(|known| known == skill) {
                skills.push(skill.to_string());
            }
        }
    }
    skills
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Parked market waits: `(index, id, since)` of every ready `market:buy`
/// obtain whose own last history entry is `awaiting_fill` (`since`: unix
/// seconds of that entry). A later entry of the node ends the wait.
pub(crate) fn parked_waits(life: &Life) -> Vec<(usize, String, i64)> {
    let mut out = Vec::new();
    for (index, node) in life.nodes.iter().enumerate() {
        let parked = node.status == NodeStatus::Ready
            && node.method.as_deref() == Some("market:buy")
            && matches!(node.kind, NodeKind::ObtainItems { .. });
        if !parked {
            continue;
        }
        let last = life.history.iter().rev().find(|entry| entry.node == node.id);
        if let Some(entry) = last.filter(|entry| entry.outcome == "awaiting_fill") {
            out.push((index, node.id.clone(), entry.ended));
        }
    }
    out
}

/// Settle every parked wait (holdings read live). Returns (the life
/// changed, ids still waiting).
pub(crate) fn settle_parked(job: &Job, life: &mut Life, deps: &crate::brain_deps::BrainDeps) -> (bool, Vec<String>) {
    let now = now_secs();
    let mut changed = false;
    let mut waiting = Vec::new();
    for (_, node_id, since) in parked_waits(life) {
        let Some(position) = life.nodes.iter().position(|node| node.id == node_id) else { continue };
        let NodeKind::ObtainItems { item, count } = life.nodes[position].kind.clone() else { continue };
        if crate::brain_await::settle_fill(job, life, position, &item, count, crate::downtime::awake_secs(since, now), deps) {
            waiting.push(node_id);
        } else {
            changed = true;
        }
    }
    if changed {
        lifescape_planner::normalize(life);
    }
    (changed, waiting)
}

/// Why a filler window found no work (typed, logged by the caller,
/// never a silent idle).
#[derive(Debug, Clone, PartialEq)]
pub enum FillerRefusal {
    /// Nothing outside the skills of the fillers that just failed is
    /// workable (`steer_for_work`).
    NoWork(String),
    /// The resolve blocked (a tool, a fare, a combat level to train).
    Blocked(String),
    /// No money method resolved at all.
    NoMethod,
}

impl std::fmt::Display for FillerRefusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FillerRefusal::NoWork(reason) => write!(out, "no filler: {reason}"),
            FillerRefusal::Blocked(reason) => write!(out, "filler blocked: {reason}"),
            FillerRefusal::NoMethod => write!(out, "no money method workable"),
        }
    }
}

/// What a filler window works: its method, the banned skill it works
/// through (every workable method banned), and whether the money guide
/// drew it.
#[derive(Debug, Clone, PartialEq)]
pub struct FillerPick {
    pub method: String,
    pub through: Option<String>,
    pub from_guide: bool,
}

/// Last refused filler attempt per account (unix seconds): the next
/// attempt waits `FILLER_REFUSAL_TICKS` (review lot 3, point 6: each
/// attempt reads the money guide twice, alchemy and money).
fn refusals() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, i64>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// True while `account`'s last refused filler is under
/// `FILLER_REFUSAL_TICKS` old at `now` (unix seconds).
pub(crate) fn refused_recently(account: &str, now: i64) -> bool {
    refusals().get(account).is_some_and(|since| crate::brain_stuck::seconds_to_ticks(now - since) < FILLER_REFUSAL_TICKS)
}

/// A filler attempt of `account` was refused at `now` (unix seconds).
pub(crate) fn note_refusal(account: &str, now: i64) {
    refusals().insert(account.to_string(), now);
}

/// A filler of `account` launched: the next attempt never waits.
pub(crate) fn clear_refusal(account: &str) {
    refusals().remove(account);
}

/// True for a money line a filler window works on its own, at once
/// (review lot 3): class 0 of `money_pick::class_of` only (tools held,
/// food in hand where the spot bites: a class 1 fight without food stops
/// `food_blocked` after the bank, a class 2 line blocks on its tool and
/// the worker idles the whole wait); never a 100 % market lot (its
/// purchases are graph nodes a window never places); never a skill of
/// the fillers that just failed (`failed`: the strict exclusion of
/// `steer_for_work`, a guide of that skill alone would leave no work).
/// Quarantined, cooled or quest-gated lines are never drawn anyway.
pub(crate) fn window_works(line: &GuideLine, ctx: &ReadyCtx, failed: &[String]) -> bool {
    !lifescape_planner::trade_craft::is_trade(&line.method)
        && !lifescape_planner::method_banned(&line.method, failed)
        && lifescape_planner::money_pick::class_of(line, ctx) == Some(0)
}

/// Resolves a filler window's coin node on `ctx` (levels, tools, wallet,
/// position, market, quests and draw seed already filled) like a money
/// node (plan équilibre step 3): `publish` hands the money guide's lines
/// (the brain's `brain_money::publish`, each line's profit read on the
/// shared demand at this pick), only the lines a window works on its own
/// stay (`window_works`), `money_pick` draws. No drawable line: the
/// static money methods rank, as for every money node (docs/planner.md,
/// R7), logged by the caller (`FillerPick::from_guide` false). Then the
/// window steers off the bans (`steer_for_work`).
pub fn pick_window(node: &mut Node, ctx: &mut ReadyCtx, publish: &mut dyn FnMut(&mut ReadyCtx, &Node), banned: &[String], failed: &[String]) -> Result<FillerPick, FillerRefusal> {
    publish(ctx, node);
    let lines: Vec<GuideLine> = ctx.money_guide.iter().filter(|line| window_works(line, ctx, failed)).cloned().collect();
    ctx.money_guide = lines;
    ctx.drawn.borrow_mut().take();
    lifescape_planner::resolve_ready(node, ctx);
    let drawn = ctx.drawn.borrow().as_ref().map(|choice| choice.method.clone());
    let through = match lifescape_planner::steer_for_work(node, ctx, banned, failed) {
        Ok(lifescape_planner::WorkSteer::ThroughBan(skill)) => Some(skill),
        Ok(lifescape_planner::WorkSteer::OffBans(_)) => None,
        Err(reason) => return Err(FillerRefusal::NoWork(reason)),
    };
    match (&node.status, node.method.clone()) {
        (NodeStatus::Blocked { reason }, _) => Err(FillerRefusal::Blocked(reason.clone())),
        (_, Some(method)) if !method.is_empty() => {
            // A ban steering may move the window off the drawn line.
            let from_guide = drawn.as_deref() == Some(method.as_str());
            Ok(FillerPick { method, through, from_guide })
        }
        _ => Err(FillerRefusal::NoMethod),
    }
}

/// The coin node of the window filling `first`'s wait and its resolve
/// context, like a money node's (levels, tools, wallet, position, the
/// fleet's sites, market, quests, its own draw seed).
pub fn window_ctx(job: &Job, life: &Life, live: &Value, first: &str) -> (Node, ReadyCtx) {
    window_ctx_in(job, life, live, first, &crate::brain_deps::BrainDeps::live())
}

/// `window_ctx` reading through the pass's handles (lot 3.5).
pub(crate) fn window_ctx_in(job: &Job, life: &Life, live: &Value, first: &str, deps: &crate::brain_deps::BrainDeps) -> (Node, ReadyCtx) {
    let coins = crate::brain_plan::holdings_count(job, "coins", deps);
    let node = Node::new(&format!("fill-{first}"), NodeKind::HaveCoins { amount: coins.saturating_add(1) }, Vec::new(), 0);
    let mut ctx = crate::brain_plan::ready_ctx_in(life, &node, live, deps);
    ctx.coins = coins;
    ctx.tools = crate::brain_plan::carried_tools(job, deps);
    ctx.bank_known = live.get("bank_streamed").as_bool() == Some(true);
    ctx.combat = crate::brain_plan::live_combat(live);
    ctx.position = live.get("position").as_arr().and_then(|cells| {
        let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
        Some((part(1)?, part(2)?, part(0)?))
    });
    ctx.crowd = deps.crowd.others(&job.account);
    ctx.seats = deps.crowd.seats(&job.account);
    crate::brain_market::fill_ctx_in(&mut ctx, &node, live, deps);
    // Like a money node (plan équilibre step 3): the quests it finished
    // gate the guide's lines, its draw has its own seed.
    ctx.quests_done = crate::brain_rescue::quests_done(life);
    ctx.draw_seed = lifescape_planner::guide_pick::seed_for(life.profile.seed, &node.id, life.memory.draws);
    (node, ctx)
}

/// The money guide's lines of a window (alchemy of its stock priced for
/// the static ranking too), never the 100 % market lots (`window_works`).
/// What the bot keeps is read once for both (lot 3 review, CPU: twice
/// before): the window's node is never in the graph, so its
/// `kept_except` is the alchemy's `kept`.
pub fn publish_window(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, job: &Job) {
    publish_window_in(ctx, node, life, live, job, &crate::brain_deps::BrainDeps::live());
}

/// `publish_window` reading through the pass's handles (lot 3.5).
pub(crate) fn publish_window_in(ctx: &mut ReadyCtx, node: &Node, life: &Life, live: &Value, job: &Job, deps: &crate::brain_deps::BrainDeps) {
    let keep = crate::brain_alch::kept_except(life, live, Some(&node.id));
    crate::brain_alch::publish_keeping(ctx, node, life, live, &keep, deps);
    crate::brain_money::publish_keeping(ctx, node, life, live, job, &keep, deps);
}

/// One filler window while `waiting` rests on the book and the graph has
/// nothing else ready. Returns true when a run was queued (the caller
/// saved the life first). Never through a ban; nothing workable is
/// logged (typed reason), never a silent idle.
pub(crate) fn launch_filler(orch: &Arc<dyn Fleet>, job: &Arc<Job>, life: &mut Life, live: &Value, waiting: &[String], deps: &crate::brain_deps::BrainDeps) -> bool {
    let Some(first) = waiting.first() else { return false };
    // A filler just queued: the worker may still read idle (one run at a time).
    if crate::brain_launch::recent_launch(&job.account) {
        return false;
    }
    // A refused attempt is not read again every pass (its reason is
    // logged once, `note_idle`).
    if refused_recently(&job.account, now_secs()) {
        return false;
    }
    let (mut node, mut ctx) = window_ctx_in(job, life, live, first, deps);
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now_secs() - stamp);
    let banned = lifescape_planner::banned_skills(&life.history, &ticks_since);
    // A filler that just failed (or ended without working) sits out,
    // strictly; a goal ban is avoided when anything else works, never a
    // reason to idle (2026-11-14, george_97: "no filler" for a whole ban).
    let failed = failed_filler_skills(life, &ticks_since);
    let reader: &Life = life;
    let mut publish = |ctx: &mut ReadyCtx, node: &Node| publish_window_in(ctx, node, reader, live, job, deps);
    let picked = pick_window(&mut node, &mut ctx, &mut publish, &banned, &failed);
    let pick = match picked {
        Ok(pick) => pick,
        Err(refusal) => {
            note_idle(life, &format!("{} awaiting its fill, {refusal}", waiting.join(",")));
            note_refusal(&job.account, now_secs());
            return false;
        }
    };
    if !pick.from_guide {
        crate::logln!("brain {}: {} no money guide line a window works now ({} kept): static money method {}", job.account, node.id, ctx.money_guide.len(), pick.method);
    }
    let (method, through) = (pick.method, pick.through);
    let Some((script, mut args)) = lifescape_planner::map_method(&method, &node, Some((0, FILLER_TICKS))) else {
        note_idle(life, &format!("{} awaiting its fill, filler {method} maps to no script", waiting.join(",")));
        note_refusal(&job.account, now_secs());
        return false;
    };
    if !crate::registry::script_known(&script) {
        note_idle(life, &format!("{} awaiting its fill, filler script {script} unknown", waiting.join(",")));
        note_refusal(&job.account, now_secs());
        return false;
    }
    clear_refusal(&job.account);
    crate::site_crowd::note(&job.account, node.site, &method);
    // The guide's choice, logged and kept under the kind `filler` (its
    // own pool; the draw counter moves on: the next window draws again).
    crate::brain_picks::record_filler(life, &node, &ctx, now_secs());
    // Its product goes on the public order board until its report (plan
    // équilibre step 2).
    crate::brain_money::claim_launch(&job.account, &node, &method, Some((0, FILLER_TICKS)), live, deps);
    let through = through.map(|skill| format!(" through the {skill} ban (every workable money method is banned)")).unwrap_or_default();
    crate::logln!("brain {}: {} awaiting its fill, working {method} meanwhile{through} ({FILLER_TICKS} ticks): {script} {}", job.account, waiting.join(","), args.join(" "));
    push_history(life, &node.id, &format!("{FILLER_LAUNCH_TAG}{}", lifescape_planner::cooldown::cool_key(&method, node.site)));
    if let Err(error) = crate::brain::save_life(life) {
        crate::logln!("{error}");
    }
    crate::brain_launch::note_launch(&job.account);
    args.push(crate::report_meta::METHOD_FLAG.to_string());
    args.push(method.clone());
    let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
    let reply = orch.run_job(&job.account, &script, &values);
    if reply.get("ok").as_bool() != Some(true) {
        crate::logln!("brain {}: filler launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
        // No window, no units on their way (review lot 2, point 2).
        crate::market_claims::release_run(&job.account, &node.id);
    }
    true
}

/// Last "no filler" reason logged per account (one line per change).
fn note_idle(life: &Life, text: &str) {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    let mut table = TABLE.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if table.get(&life.account).map(String::as_str) != Some(text) {
        crate::logln!("brain {}: {text}", life.account);
        table.insert(life.account.clone(), text.to_string());
    }
}

#[cfg(test)]
#[path = "brain_filler_tests.rs"]
mod tests;
