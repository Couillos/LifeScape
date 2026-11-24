//! Brain report folding: one pushed task report moves one node through
//! `lifescape_planner::lifecycle` (the only place that changes statuses),
//! then the graph is normalized and saved. Launching stays in brain.rs.
//!
//! Classification reads the report's own typed `error` (never a
//! session-wide `last_error` that may belong to an older run):
//! - `Usage` (bad CLI, dispatch the script can never serve): abandoned at
//!   once, the same launch would fail the same way;
//! - `Failed`/`Sdk`/`exit`, death, lost stock: counted, retried with a
//!   fresh resolve until `MAX_FAILURES`, then abandoned;
//! - travel failure: the unreachable method is banned (never relaunched
//!   as is) and the failure counted;
//! - `blocked` with a typed `missing`: one obtain sub-task per item,
//!   reused when already open, bounded by `MAX_BLOCKS`;
//! - `stopped` (operator unload, daemon restart): back to ready, uncounted.

#![forbid(unsafe_code)]

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{complete_node, end_engagement, normalize, record_contract_error, record_failure_timed, retry_uncounted, FailureFold, NodeKind, NodeStatus};

use crate::brain::{load_life, push_history, save_life};
use crate::brain_plan::{fold_travel_failure, holdings_count, is_travel_failure, rescue_replan_cause, TravelFold};
use crate::fleet::Fleet;
use crate::jobs::Job;

fn reach_level(report: &Value, skill: &str) -> Option<u8> {
    let stat = lifescape_sdk::provenance::skill_id(&skill.to_lowercase())? as i64;
    report
        .get("levels_after")
        .as_arr()?
        .iter()
        .find(|row| row.get("stat").as_i64() == Some(stat))
        .and_then(|row| row.get("level").as_i64())
        .map(|level| level.clamp(1, 99) as u8)
}

/// Combat level from the report's `levels_after` (XP-max formula).
fn reach_combat(report: &Value) -> Option<u8> {
    let rows = report.get("levels_after").as_arr()?;
    let stat = |id: i64| rows.iter().find(|row| row.get("stat").as_i64() == Some(id)).and_then(|row| row.get("level").as_i64()).unwrap_or(1).clamp(1, 99) as u8;
    Some(lifescape_planner::combat_level(&lifescape_planner::BaseLevels {
        attack: stat(0),
        defence: stat(1),
        strength: stat(2),
        hitpoints: stat(3).max(10),
        ranged: stat(4),
        prayer: stat(5),
        magic: stat(6),
    }))
}

fn report_missing(report: &Value) -> Vec<String> {
    report
        .get("missing")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// Typed error carried by the report: (`type`, `message`).
pub(crate) fn report_error(report: &Value) -> Option<(String, String)> {
    let error = report.get("error");
    let kind = error.get("type").as_str()?.to_string();
    Some((kind, error.get("message").as_str().unwrap_or("").to_string()))
}

/// What one report does to its node (pure decision, applied by
/// `handle_report`; unit-tested without a daemon).
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    /// Goal reached: the node is done.
    Done,
    /// Market order placed but not filled yet: keep waiting (bounded).
    AwaitFill,
    /// Engagement window spent: money closes, levels continue.
    EngagementEnd,
    /// Operator stop or daemon restart: retry later, uncounted.
    Stopped,
    /// Typed missing requirements: sub-tasks.
    Blocked(Vec<String>),
    /// Contract error: abandon now.
    Contract(String),
    /// Unreachable method: ban it, count the failure.
    Travel(String),
    /// Runtime failure: count it.
    Failure(String),
}

/// XP a report credits toward `kind`: the climbed skill of a
/// `reach_level`, every combat skill of a `reach_combat`, 0 otherwise.
pub(crate) fn xp_toward(report: &Value, kind: &NodeKind) -> i64 {
    const COMBAT: [&str; 6] = ["attack", "strength", "defence", "hitpoints", "ranged", "magic"];
    let counts = |skill: &str| match kind {
        NodeKind::ReachLevel { skill: climbed, .. } => climbed.eq_ignore_ascii_case(skill),
        NodeKind::ReachCombat { .. } => COMBAT.contains(&skill.to_lowercase().as_str()),
        _ => false,
    };
    report
        .get("xp")
        .as_arr()
        .map(|rows| rows.iter().filter(|row| row.get("skill").as_str().is_some_and(counts)).filter_map(|row| row.get("xp").as_i64()).filter(|gain| *gain > 0).sum())
        .unwrap_or(0)
}

/// Units of the node's own target the run gained, read in the report
/// itself (tour 2 P2-8): the `items` deltas, or the coins a money node
/// counts. The fold judges a run by what IT brought, not only by the live
/// holdings, which may lag the report (will_field 12:36: stringing
/// `--want strung_emerald_amulet 1` made its amulet, the brain read
/// "short") or measure a target no single run meets (ella_quill 12:29: an
/// alchemy run on a `have_coins` node, +1 216 coins, "short").
pub(crate) fn gained_toward(report: &Value, kind: &NodeKind) -> i64 {
    let wanted = match kind {
        NodeKind::ObtainItems { item, .. } => item.as_str(),
        NodeKind::HaveCoins { .. } | NodeKind::MakeMoney => "coins",
        _ => return 0,
    };
    // Report rows carry the display name ("Coins", "Yew logs"), shared by
    // homonyms ("Emerald amulet": strung and unstrung): a display row is
    // the target only when the run's method names it so (`debug_in`, the
    // guides' rule, tour 3 point 7).
    let display = lifescape_gamedata::objs_by_name(wanted).first().and_then(|obj| obj.display);
    let method = report.get("planner_method").as_str().unwrap_or("");
    let named = |name: &str| name == wanted || (Some(name) == display && crate::rates_names::debug_in(method, name, false) == wanted);
    report
        .get("items")
        .as_arr()
        .map(|rows| rows.iter().filter(|row| row.get("name").as_str().is_some_and(named)).filter_map(|row| row.get("qty").as_i64()).filter(|qty| *qty > 0).sum())
        .unwrap_or(0)
}

/// A coin node (`MakeMoney`, `HaveCoins`) working a 100 % market craft
/// lot (`trade:craft/..`) is paid when the products SELL, not when they
/// are made: a lot that made products is progress, never "goal check is
/// short" (live 2026-11-22: 208 such failures, nodes abandoned, skills
/// banned, e.g. 31 steel chainbodies smithed on lily_north). The next
/// pass checks the wallet.
pub(crate) fn coin_lot_progress(kind: &NodeKind, method: &str, made: i64) -> bool {
    matches!(kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) && made > 0 && lifescape_planner::trade_craft::is_trade(method)
}

/// History outcome of a node folded done: a janitor pass that exits on
/// its budget (`market cleanup` exits so only while the bank is still at
/// or past the limit) is short, and holds the next insert back
/// (`space::cleanup_cooling`); everything else reached its goal.
pub(crate) fn done_outcome(kind: &NodeKind, stop: &str) -> &'static str {
    if matches!(kind, NodeKind::SpaceCleanup) && stop != "goal_reached" {
        lifescape_planner::CLEANUP_SHORT_TAG
    } else {
        "goal_reached"
    }
}

/// Decide from the stop, the typed error, missing items, and what the
/// goal check says (`goal_met`, `market_pending`, `progressed`: the run
/// earned XP toward the climb or items toward the obtain).
pub(crate) fn verdict(stop: &str, error: Option<(String, String)>, missing: &[String], goal_met: bool, market_pending: bool, progressed: bool) -> Verdict {
    let text = error.as_ref().map(|(kind, message)| format!("{kind}: {message}")).unwrap_or_default();
    match stop {
        "goal_reached" if goal_met => Verdict::Done,
        "goal_reached" if market_pending => Verdict::AwaitFill,
        // A run that ended on its own budget short of the goal but with
        // progress (prayer at cows: +652 XP a window) continues
        // uncounted, like a spent engagement window (live 2026-11-13:
        // three such windows banned prayer on frank_lane and leo_king).
        "goal_reached" if progressed => Verdict::EngagementEnd,
        "goal_reached" => Verdict::Failure("goal_reached but the goal check is short (method ended without the items/level)".to_string()),
        // A window can end exactly when the goal lands (level reached,
        // stock farmed): that is a success, not one more window.
        "budget_exhausted" if goal_met => Verdict::Done,
        "budget_exhausted" => Verdict::EngagementEnd,
        "stopped" => Verdict::Stopped,
        "blocked" if !missing.is_empty() => Verdict::Blocked(missing.to_vec()),
        "blocked" => Verdict::Failure(format!("blocked without a typed missing item ({text})")),
        _ => match error {
            Some((kind, _)) if kind == "Usage" => Verdict::Contract(text),
            // Deaths at a spot (the fight's own guard, `survival.rs`): the
            // spot is out of the bot's league, the skill sits out at once
            // instead of three more deaths (live 2026-11-15: 33 bots sent
            // back to guards, `melee guards --want grain`).
            Some((_, message)) if message.contains("too dangerous after") => Verdict::Contract(text),
            Some((_, message)) if is_travel_failure(&lifescape_json::object(vec![("message", lifescape_json::str_value(&message))])) => Verdict::Travel(text),
            Some(_) => Verdict::Failure(text),
            None if missing.iter().any(|need| need.contains("died") || need.contains("death")) => Verdict::Failure("died".to_string()),
            None => Verdict::Failure(format!("stop {stop} without a typed error")),
        },
    }
}

/// Budget windows in a row without progress toward a measurable node
/// (climb XP, obtained items) before the node counts a failure: the brain
/// never relaunches a method that does nothing forever (live 2026-11-14:
/// pottery n133 relaunched ~115 times, 689 124 ticks, 0 dish).
pub(crate) const IDLE_WINDOWS_LIMIT: u32 = 3;
/// History outcome of a budget window without progress toward its node.
pub(crate) const IDLE_OUTCOME: &str = "engagement_idle";

/// `verdict` plus the silent-loop rules: a run the detector flags
/// (`report_progress::assess`: no output, bank churn) fails its node at
/// once; a budget window without progress toward the node is the
/// `idle_windows + 1`-th in a row and fails it at `IDLE_WINDOWS_LIMIT`.
/// A goal met wins over both.
#[allow(clippy::too_many_arguments)]
pub(crate) fn verdict_watched(
    stop: &str,
    error: Option<(String, String)>,
    missing: &[String],
    goal_met: bool,
    market_pending: bool,
    progressed: bool,
    silent: Option<&str>,
    idle_windows: Option<u32>,
) -> Verdict {
    let window_end = stop == "budget_exhausted" || (stop == "goal_reached" && !market_pending);
    if window_end && !goal_met {
        if let Some(text) = silent {
            return Verdict::Failure(text.to_string());
        }
        if let Some(before) = idle_windows.filter(|_| !progressed && stop == "budget_exhausted") {
            if before + 1 >= IDLE_WINDOWS_LIMIT {
                return Verdict::Failure(format!("no progress: {} budget windows in a row without progress toward the node", before + 1));
            }
        }
    }
    verdict(stop, error, missing, goal_met, market_pending, progressed)
}

/// Budget windows without progress the node ended just before this one
/// (latest first, operator stops skipped).
pub(crate) fn idle_windows(life: &lifescape_planner::Life, node: &str) -> u32 {
    life.history.iter().rev().filter(|entry| entry.node == node && entry.outcome != "stopped").take_while(|entry| entry.outcome == IDLE_OUTCOME).count() as u32
}

/// The node's last error as the fold line prints it (tour 2 P2-11): only
/// when THIS report failed or blocked the node. A success, an engagement
/// end or a stop never reprints an older error (leo_king's "unknown
/// server opcode 149" echoed every 6 min, 22 such lines for 0 incident).
pub(crate) fn fresh_error<'a>(decision: &Verdict, last_error: &'a str) -> &'a str {
    match decision {
        Verdict::Failure(_) | Verdict::Contract(_) | Verdict::Travel(_) | Verdict::Blocked(_) => last_error,
        Verdict::Done | Verdict::AwaitFill | Verdict::EngagementEnd | Verdict::Stopped => "",
    }
}

/// Fold one pushed report into the graph; launches nothing (the launch
/// pass runs right after, same brain tick).
/// The node a report names (empty: none). Its run is over whatever the
/// fold decides next (ignored, retired, unknown, folded): its claim on the
/// order board ends here, before any early return (plan équilibre step
/// 2, review lot 2 point 2).
pub(crate) fn reported_node(account: &str, report: &Value) -> String {
    let node_id = report.get("node_id").as_str().unwrap_or("").to_string();
    if !node_id.is_empty() {
        crate::market_claims::release_run(account, &node_id);
    }
    node_id
}

pub(crate) fn handle_report(orch: &Arc<dyn Fleet>, job: &Arc<Job>, report: &Value) {
    let deps = &crate::brain_deps::BrainDeps::live();
    let account = job.account.clone();
    let node_id = reported_node(&account, report);
    let Some(mut life) = load_life(&account) else { return };
    if node_id.is_empty() {
        return;
    }
    let Some(index) = intake(&account, &mut life, &node_id, report) else { return };
    let stop = report.get("stop").as_str().unwrap_or("").to_string();
    // A foodless fight obtains the best food its wallet buys (`brain_food`).
    let fight_method = life.nodes[index].method.clone().unwrap_or_default();
    let missing = crate::brain_food::afford_food_needs(job, &fight_method, report_missing(report), deps);
    let ticks = report.get("ticks").as_i64().unwrap_or(0).max(0) as u64;
    life.nodes[index].spent_ticks = life.nodes[index].spent_ticks.saturating_add(ticks);
    // A rescue that ran out of budget mid-script is typed in the
    // session's last_error only (P1.6): read it when the report carries
    // no error of its own.
    let mut error = report_error(report);
    if error.is_none() && stop == "failed" {
        let rescue = job.inner.try_lock().ok().and_then(|inner| inner.last_error.as_ref().and_then(rescue_replan_cause));
        if rescue.is_some() {
            error = Some(("RescueBudget".to_string(), "random event rescue ran out of budget".to_string()));
        }
    }
    let kind = life.nodes[index].kind.clone();
    let method = life.nodes[index].method.clone().unwrap_or_default();
    let site = life.nodes[index].site;
    // A 100 % market lot's craft run that spent its bought inputs with
    // products made finished its lot (R5): never a block that buys again.
    let made = crate::brain_trade::made_of(report, &method);
    let stop = if lifescape_planner::trade_craft::lot_finished(&method, &missing, made) {
        crate::logln!("brain {account}: {node_id} trade lot spent its inputs ({made} made), lot done");
        "goal_reached".to_string()
    } else {
        stop
    };
    // XP the run earned toward a climb is progress even short of a level
    // (a food refill is not a dead block; a slow prayer window is no
    // failure).
    let xp = xp_toward(report, &kind);
    lifescape_planner::record_xp(&mut life, index, xp);
    let run = Run { orch, job, report, deps, account, index, id: life.nodes[index].id.clone(), kind, method, site, ticks, made };
    let held_before = life.nodes[index].progress_current;
    let (goal_met, market_pending) = run.goal_state(&mut life);
    let progressed = xp > 0
        || (matches!(run.kind, NodeKind::ObtainItems { .. }) && life.nodes[index].progress_current > held_before)
        || gained_toward(report, &run.kind) > 0
        || coin_lot_progress(&run.kind, &run.method, made);
    // Silent loops: the report-level detector (no output, bank churn),
    // then windows without progress toward a measurable node.
    let silent = crate::report_progress::assess(report).map(|finding| finding.message);
    let measured = matches!(run.kind, NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. } | NodeKind::ObtainItems { .. });
    let idle_before = measured.then(|| idle_windows(&life, &node_id));
    let decision = verdict_watched(&stop, error, &missing, goal_met, market_pending, progressed, silent.as_deref(), idle_before);
    if let (Verdict::Failure(text), None) = (&decision, &silent) {
        if text.starts_with("no progress") {
            crate::logln!("brain {}: {node_id} {text}", run.account);
        }
    }
    run.record_method(&mut life, &decision);
    run.apply(&mut life, &decision, &stop, progressed, measured);
    run.finish(&mut life, &decision);
}

/// Which node a report folds into, or `None` when it folds nowhere: a
/// filler window (`fill-<id>`) lives outside the graph (its report lands
/// in the history, a failed one keeps its skill out of the next fillers:
/// never "unknown node, ignored", that relaunched a 0-tick filler in a
/// loop), a retired or unknown node, a node the graph already moved
/// (crash recovery counted it, operator reset: never fold twice).
fn intake(account: &str, life: &mut lifescape_planner::Life, node_id: &str, report: &Value) -> Option<usize> {
    if crate::brain_filler::is_filler_node(node_id) {
        let line = crate::brain_filler::fold_filler_report(life, node_id, report);
        crate::logln!("brain {account}: {line}");
        if save_life(life).is_err() {
            crate::logln!("brain {account}: life save failed, filler report dropped");
        }
        return None;
    }
    let Some(index) = life.nodes.iter().position(|node| node.id == node_id) else {
        // Finished nodes retire from the graph (`lifecycle_retire`); ids
        // are never reused, so a late report names a retired node.
        let retired = lifescape_planner::lifecycle_retire::node_number(node_id).is_some_and(|number| number <= lifescape_planner::max_id_number(life));
        let what = if retired { "retired" } else { "unknown" };
        crate::logln!("brain {account}: report for {what} node {node_id}, ignored");
        return None;
    };
    if life.nodes[index].status != NodeStatus::Active {
        crate::logln!("brain {account}: report for {node_id} which is {}, ignored", life.nodes[index].status.as_str());
        return None;
    }
    Some(index)
}

/// One report being folded into one node: what the fold reads, and one
/// method per kind of verdict (`apply` picks it).
struct Run<'a> {
    orch: &'a Arc<dyn Fleet>,
    job: &'a Arc<Job>,
    report: &'a Value,
    deps: &'a crate::brain_deps::BrainDeps,
    account: String,
    index: usize,
    id: String,
    kind: NodeKind,
    method: String,
    site: Option<(i64, i64, i64)>,
    ticks: u64,
    made: i64,
}

impl Run<'_> {
    /// Is the node's goal met by this report, and is a market buy still
    /// pending (the fold then waits for the fill).
    fn goal_state(&self, life: &mut lifescape_planner::Life) -> (bool, bool) {
        let (index, report, job, deps) = (self.index, self.report, self.job, self.deps);
        match &self.kind {
            NodeKind::ReachLevel { skill, level } => match reach_level(report, skill) {
                Some(reached) => {
                    // Progress clears the block count (one input batch per
                    // block is the normal rhythm of a transformation skill).
                    lifescape_planner::record_progress(life, index, i64::from(reached));
                    (reached >= *level, false)
                }
                None => (true, false),
            },
            NodeKind::ObtainItems { item, count } => {
                let quota = matches!(&life.goal, lifescape_planner::GoalKind::Produce { item: made, .. } if made == item);
                let held = if quota { holdings_count(job, item, deps) } else { lifescape_planner::tools::family_held(item, *count, &|name| holdings_count(job, name, deps)) };
                life.nodes[index].progress_current = held;
                (held >= *count, self.method == "market:buy")
            }
            // A coin target is met by the wallet, whatever the method sold.
            NodeKind::HaveCoins { amount } => {
                let held = holdings_count(job, "coins", deps);
                life.nodes[index].progress_current = held;
                (held >= *amount, false)
            }
            // A training node is done when the combat level is reached, not
            // when one window ends (the dependent re-picks its site then).
            NodeKind::ReachCombat { level } => match reach_combat(report) {
                Some(reached) => {
                    lifescape_planner::record_progress(life, index, i64::from(reached));
                    (reached >= *level, false)
                }
                None => (false, false),
            },
            _ => (true, false),
        }
    }

    /// What the lot's craft run made (R5) and the method's health
    /// (`method_health`): what the method itself did.
    fn record_method(&self, life: &mut lifescape_planner::Life, decision: &Verdict) {
        if lifescape_planner::trade_craft::is_trade(&self.method) && matches!(decision, Verdict::Done | Verdict::EngagementEnd) {
            let work = self.report.get("work_ticks").as_i64().unwrap_or(self.ticks as i64);
            lifescape_planner::trade_craft::note_made(&mut life.memory.trades, &self.id, self.made, work);
        }
        let (account, method) = (&self.account, &self.method);
        match decision {
            Verdict::Done | Verdict::EngagementEnd => crate::method_health::record(account, method, crate::method_health::Outcome::Ok, ""),
            Verdict::Failure(text) | Verdict::Contract(text) | Verdict::Travel(text) => crate::method_health::record(account, method, crate::method_health::Outcome::Fail, text),
            Verdict::AwaitFill | Verdict::Stopped | Verdict::Blocked(_) => {}
        }
    }

    /// The graph move of one verdict.
    fn apply(&self, life: &mut lifescape_planner::Life, decision: &Verdict, stop: &str, progressed: bool, measured: bool) {
        match decision {
            Verdict::Done => self.on_done(life, stop),
            Verdict::AwaitFill => {
                // Stays active (the lock holds) until the holdings check in
                // the launch pass closes it or its deadline fails it.
                push_history(life, &self.id, "awaiting_fill");
            }
            Verdict::EngagementEnd => {
                end_engagement(life, self.index);
                push_history(life, &self.id, if measured && !progressed { IDLE_OUTCOME } else { "engagement_end" });
            }
            Verdict::Stopped => {
                retry_uncounted(life, self.index);
                push_history(life, &self.id, "stopped");
            }
            Verdict::Blocked(needs) => self.on_blocked(life, needs),
            Verdict::Contract(text) => self.on_contract(life, text),
            Verdict::Travel(text) => self.on_travel(life, text),
            Verdict::Failure(text) => self.on_failure(life, text),
        }
    }

    fn on_done(&self, life: &mut lifescape_planner::Life, stop: &str) {
        // D-B: whichever leg landed first, the other one closes.
        if let NodeKind::ObtainItems { item, .. } = &self.kind {
            crate::brain_market::cancel_open_buys(&self.account, item, self.deps);
        }
        let index = self.index;
        let proof = life.nodes[index].progress_target.max(life.nodes[index].progress_current);
        complete_node(life, index, proof);
        push_history(life, &self.id, done_outcome(&self.kind, stop));
    }

    fn on_blocked(&self, life: &mut lifescape_planner::Life, needs: &[String]) {
        let (index, id, method, account) = (self.index, &self.id, &self.method, &self.account);
        // Every node of the site depleted and another site sooner (user
        // 2026-11-20): the site cools for its respawn, no failure, no
        // block counted, no ban; the next resolve picks the other site.
        if crate::brain_cool::depleted_site(needs).is_some() {
            let (depleted, ticks) = crate::brain_cool::depleted_site(needs).unwrap_or(((0, 0, 0), 0));
            crate::brain_cool::on_depleted(life, method, self.site.unwrap_or(depleted), ticks);
            retry_uncounted(life, index);
            push_history(life, id, "depleted");
            return;
        }
        // A contended site (D4, user 2026-11-17): no failure, no block
        // counted, no ban; the site cools for this bot and the next
        // resolve sends it elsewhere at once.
        if crate::brain_cool::contended_site(needs).is_some() {
            let contended = crate::brain_cool::contended_site(needs).unwrap_or((0, 0, 0));
            let cooled_site = self.site.unwrap_or(contended);
            crate::brain_cool::on_contended(life, method, cooled_site);
            retry_uncounted(life, index);
            push_history(life, id, "contended");
            return;
        }
        // A need its own listings lock (P5, `locked:<object>`): the
        // listings go, the node relaunches, no block counted, no buy.
        let locked = crate::brain_tools::locked_needs(needs);
        let withdrawn = crate::brain_tools::release_own_locks(&**self.orch, account, &locked, self.deps);
        if withdrawn > 0 {
            crate::logln!("brain {account}: {id} blocked on its own listings ({}): {withdrawn} listing(s) withdrawn, relaunch", locked.join(", "));
            retry_uncounted(life, index);
            push_history(life, id, "locked_released");
            return;
        }
        // A missing level climbs from the reported live level. A
        // lock no withdrawal freed (a deal in flight) is the plain
        // need. The block reads the measured plan (R7).
        let needs = crate::brain_tools::plain_needs(needs);
        let report = self.report;
        let level_of = |skill: &str| reach_level(report, skill).unwrap_or(1);
        let live = self.job.inner.try_lock().map(|inner| inner.live_or_null()).unwrap_or_else(|_| crate::jobs::null_live());
        let view = crate::brain_plan_read::BotView::of(life);
        // A load that ran dry after making units is a stock-out:
        // the obtain is filed as ever, the block never counts
        // (cleared before, so the 4th never abandons; after, so
        // the count stays zero).
        let made = if crate::errors_model::is_stock_out(report, &needs) { crate::errors_model::worked_units(report) } else { 0 };
        lifescape_planner::record_made(life, index, made);
        let fold = crate::brain_plan_read::with_plan_read(&view, &live, |read| lifescape_planner::record_block_with(life, index, &needs, &level_of, read), self.deps);
        lifescape_planner::record_made(life, index, made);
        push_history(life, id, crate::brain_launch::block_outcome(&fold));
        if let FailureFold::CycleBroken(line) = &fold {
            crate::logln!("brain {account}: {line}");
        }
    }

    fn on_contract(&self, life: &mut lifescape_planner::Life, text: &str) {
        record_contract_error(life, self.index, text);
        push_history(life, &self.id, "abandoned");
        crate::brain_cool::on_abandon(life, &self.method, self.site, text);
    }

    fn on_travel(&self, life: &mut lifescape_planner::Life, text: &str) {
        let index = self.index;
        let banned = matches!(fold_travel_failure(&mut life.nodes[index], &self.method), TravelFold::NoRoute(_));
        if banned {
            record_contract_error(life, index, &format!("no reachable method left ({text})"));
            push_history(life, &self.id, "abandoned");
            crate::brain_cool::on_abandon(life, &self.method, self.site, text);
        } else {
            let fold = record_failure_timed(life, index, text, self.ticks);
            push_history(life, &self.id, if fold == FailureFold::Abandoned { "abandoned" } else { "failed" });
            if fold == FailureFold::Abandoned {
                crate::brain_cool::on_abandon(life, &self.method, self.site, text);
            }
        }
    }

    fn on_failure(&self, life: &mut lifescape_planner::Life, text: &str) {
        let fold = record_failure_timed(life, self.index, text, self.ticks);
        push_history(life, &self.id, if fold == FailureFold::Abandoned { "abandoned" } else { "failed" });
        // The method that failed cools for this bot, for every goal
        // kind (plan P1): the next resolve never relaunches it hot.
        if fold == FailureFold::Abandoned {
            crate::brain_cool::on_abandon(life, &self.method, self.site, text);
        }
    }

    /// The fold line, the graph normalised, the life saved.
    fn finish(&self, life: &mut lifescape_planner::Life, decision: &Verdict) {
        let (account, id) = (&self.account, &self.id);
        // A broken cycle may have dropped the node: read it by id.
        if let Some(node) = life.nodes.iter().find(|node| &node.id == id) {
            crate::logln!(
                "brain {account}: {id} {:?} -> {} (failures {}/{} blocks {}) {}",
                decision,
                node.status.as_str(),
                node.failures,
                lifescape_planner::MAX_FAILURES,
                node.blocks,
                fresh_error(decision, &node.last_error)
            );
        }
        for change in normalize(life) {
            crate::logln!("brain {account}: normalize {change}");
        }
        if save_life(life).is_err() {
            crate::logln!("brain {account}: life save failed, report dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{verdict, Verdict};

    /// Review lot 2, point 2: any report releases its run's claim first,
    /// even one the fold then ignores (a retired or unknown node, a node
    /// the graph already moved), so no claim outlives its run.
    #[test]
    fn a_report_releases_its_runs_claim_even_when_ignored() {
        crate::market_claims::claim_run("fold_claims_probe", "n41", "fold_claims_coal", 120, 3_600);
        let ignored = lifescape_json::parse(r#"{"node_id":"n41","stop":"budget_exhausted","ticks":6000}"#).expect("report");
        assert_eq!(super::reported_node("fold_claims_probe", &ignored), "n41");
        assert_eq!(crate::market_claims::claimed_total().get("fold_claims_coal"), None, "released before any check");
        let anonymous = lifescape_json::parse(r#"{"stop":"failed"}"#).expect("report");
        assert_eq!(super::reported_node("fold_claims_probe", &anonymous), "", "no node named");
    }

    /// A spot that killed the bot bans its skill at once (contract).
    #[test]
    fn a_deadly_spot_is_a_contract_error() {
        let error = Some(("Sdk".to_string(), "sdk: unreachable: spot guards too dangerous after 1 deaths, re-gear or pick easier foes".to_string()));
        assert!(matches!(verdict("exit", error, &[], false, false, false), Verdict::Contract(_)));
    }

    fn typed(kind: &str, message: &str) -> Option<(String, String)> {
        Some((kind.to_string(), message.to_string()))
    }

    /// t14/t20: the fishing run failed with `failed: fish failed at …`
    /// (runtime) — counted, never abandoned at the first try; a real
    /// `Usage` (CLI refused) abandons.
    #[test]
    fn runtime_failures_count_and_usage_abandons() {
        assert_eq!(
            verdict("failed", typed("Failed", "fish failed at (0, 3086, 3227)"), &[], false, false, false),
            Verdict::Failure("Failed: fish failed at (0, 3086, 3227)".to_string())
        );
        assert!(matches!(verdict("failed", typed("Usage", "fishing [count|spot...]"), &[], false, false, false), Verdict::Contract(_)));
        assert!(matches!(verdict("failed", typed("Sdk", "sdk: travel failure NoRoute seg=None"), &[], false, false, false), Verdict::Travel(_)));
        assert!(matches!(verdict("failed", None, &["died at (0,1,2)".to_string()], false, false, false), Verdict::Failure(text) if text == "died"));
        assert!(matches!(verdict("failed", None, &[], false, false, false), Verdict::Failure(_)), "an untyped failure still counts");
    }

    /// Live bdr53jee 2026-11-14: `market cleanup` left 56 types and exited
    /// on its budget; the pass is short (cools the next insert down), a
    /// drained one reached its goal, other nodes never read short.
    #[test]
    fn a_janitor_budget_exit_is_a_short_pass() {
        use super::done_outcome;
        use lifescape_planner::NodeKind;
        assert_eq!(done_outcome(&NodeKind::SpaceCleanup, "budget_exhausted"), lifescape_planner::CLEANUP_SHORT_TAG);
        assert_eq!(done_outcome(&NodeKind::SpaceCleanup, "goal_reached"), "goal_reached");
        assert_eq!(done_outcome(&NodeKind::HaveCoins { amount: 10 }, "budget_exhausted"), "goal_reached");
    }

    #[test]
    fn success_waits_and_stops() {
        assert_eq!(verdict("goal_reached", None, &[], true, false, false), Verdict::Done);
        assert_eq!(verdict("goal_reached", None, &[], false, true, false), Verdict::AwaitFill, "placed market order");
        assert!(matches!(verdict("goal_reached", None, &[], false, false, false), Verdict::Failure(_)), "short instant leg");
        assert_eq!(verdict("budget_exhausted", None, &[], false, false, false), Verdict::EngagementEnd);
        assert_eq!(verdict("budget_exhausted", None, &[], true, false, false), Verdict::Done, "goal landed with the window");
        assert_eq!(verdict("stopped", None, &[], false, false, false), Verdict::Stopped);
        assert_eq!(verdict("blocked", typed("Blocked", "x"), &["small_fishing_net".to_string()], false, false, false), Verdict::Blocked(vec!["small_fishing_net".to_string()]));
        assert!(matches!(verdict("blocked", None, &[], false, false, false), Verdict::Failure(_)), "blocked without missing counts");
    }

    /// Live 2026-11-13 (frank_lane, leo_king): `melee cows --level prayer
    /// 40` buried every bone (+652 prayer XP a window) and ended on its
    /// own budget with `goal_reached` short of 40: three such windows
    /// counted three failures and banned prayer. A window that earned
    /// XP toward the node continues (levels keep climbing), uncounted.
    #[test]
    fn a_short_window_with_progress_continues() {
        assert_eq!(verdict("goal_reached", None, &[], false, false, true), Verdict::EngagementEnd);
        assert!(matches!(verdict("goal_reached", None, &[], false, false, false), Verdict::Failure(_)), "no progress still counts");
        assert_eq!(verdict("goal_reached", None, &[], true, false, true), Verdict::Done);
    }

    /// Live 2026-11-14 (harry_page n133, pottery): every window ended on
    /// its budget with no item and no XP and folded `EngagementEnd`, 115
    /// relaunches. A flagged window now fails the node; a window that
    /// produced something but nothing toward the node is idle, and the
    /// third idle window in a row fails it too. A goal met still wins.
    #[test]
    fn silent_windows_fail_their_node() {
        use super::verdict_watched;
        let silent = Some("no progress: pottery ran 6076 ticks to its budget with 0 items, 0 XP (280 bank opens)");
        assert!(matches!(verdict_watched("budget_exhausted", None, &[], false, false, false, silent, Some(0)), Verdict::Failure(text) if text.starts_with("no progress")));
        assert_eq!(verdict_watched("budget_exhausted", None, &[], true, false, false, silent, Some(0)), Verdict::Done, "goal landed");
        assert_eq!(verdict_watched("budget_exhausted", None, &[], false, false, false, None, Some(1)), Verdict::EngagementEnd, "second idle window");
        assert!(matches!(verdict_watched("budget_exhausted", None, &[], false, false, false, None, Some(2)), Verdict::Failure(_)), "third idle window");
        assert_eq!(verdict_watched("budget_exhausted", None, &[], false, false, true, None, Some(2)), Verdict::EngagementEnd, "progress resets");
        assert_eq!(verdict_watched("budget_exhausted", None, &[], false, false, false, None, None), Verdict::EngagementEnd, "money nodes have no measure");
        assert!(matches!(verdict_watched("goal_reached", None, &[], false, false, false, silent, Some(0)), Verdict::Failure(_)));
        assert_eq!(verdict_watched("stopped", None, &[], false, false, false, silent, Some(5)), Verdict::Stopped, "operator stops never count");
    }

    /// Idle windows count back from the latest entry of the node; another
    /// outcome ends the streak, operator stops are skipped.
    #[test]
    fn idle_windows_count_the_trailing_streak() {
        use lifescape_planner::{Archetype, GoalKind, Life, Profile};
        let mut life = Life {
            schema: 1,
            account: "t00".to_string(),
            created_at: 0,
            seed: 7,
            profile: Profile::new(false, 0.5, 0.5, 7, Archetype::Producer),
            goal: GoalKind::MaxSkills { skills: vec!["crafting".to_string()], level: 10 },
            nodes: Vec::new(),
            engagement: None,
            active: None,
            history: Vec::new(),
            ambition: None,
            goal_origin: None,
            memory: Default::default(),
        };
        for (node, outcome) in [("n1", "engagement_idle"), ("n1", "engagement_end"), ("n1", "engagement_idle"), ("n2", "failed"), ("n1", "stopped"), ("n1", "engagement_idle")] {
            crate::brain::push_history(&mut life, node, outcome);
        }
        assert_eq!(super::idle_windows(&life, "n1"), 2);
        assert_eq!(super::idle_windows(&life, "n2"), 0);
    }

    /// XP the report credits to the node's skill (or any combat skill for
    /// a combat level), read from the report's `xp` rows.
    /// Tour 2 P2-8 red test: a `goal_reached` run that brought units of
    /// the node's own target (the amulet strung, the coins an alchemy run
    /// paid) is progress, never "goal check is short"; the next pass
    /// closes the node on its holdings.
    #[test]
    fn a_run_that_brought_its_target_is_progress() {
        let row = |name: &str, qty: i64| lifescape_json::object(vec![("name", lifescape_json::str_value(name)), ("qty", lifescape_json::Value::Int(qty))]);
        let report = |rows: Vec<lifescape_json::Value>| lifescape_json::object(vec![("items", lifescape_json::Value::Arr(rows))]);
        let strung = report(vec![row("strung_emerald_amulet", 1), row("ball_of_wool", -1)]);
        let obtain = lifescape_planner::NodeKind::ObtainItems { item: "strung_emerald_amulet".to_string(), count: 8 };
        assert_eq!(super::gained_toward(&strung, &obtain), 1);
        let alch = report(vec![row("Coins", 1216), row("Bronze platebody", -19)]);
        assert_eq!(super::gained_toward(&alch, &lifescape_planner::NodeKind::HaveCoins { amount: 40_000 }), 1216);
        assert_eq!(super::gained_toward(&alch, &obtain), 0, "another object is no progress");
        // Tour 3 point 7: the display row "Emerald amulet" is the strung
        // amulet for the stringing run, the UNSTRUNG one for a jewelry
        // smelt: homonyms are told apart by the run's method.
        let shown = |method: &str| lifescape_json::object(vec![("planner_method", lifescape_json::str_value(method)), ("items", lifescape_json::Value::Arr(vec![row("Emerald amulet", 1)]))]);
        assert_eq!(super::gained_toward(&shown("string:unstrung_emerald_amulet"), &obtain), 1);
        let unstrung = lifescape_planner::NodeKind::ObtainItems { item: "unstrung_emerald_amulet".to_string(), count: 8 };
        assert_eq!(super::gained_toward(&shown("string:unstrung_emerald_amulet"), &unstrung), 0, "the stringing run consumed it");
        let progressed = super::gained_toward(&strung, &obtain) > 0;
        assert_eq!(verdict("goal_reached", None, &[], false, false, progressed), Verdict::EngagementEnd);
        assert!(matches!(verdict("goal_reached", None, &[], false, false, false), Verdict::Failure(_)), "nothing brought stays short");
    }

    /// Live 2026-11-22 (lily_north n1771, 31 steel chainbodies smithed
    /// for a coin node): a craft lot's products are progress of its coin
    /// node, the sale pays later.
    #[test]
    fn a_coin_nodes_craft_lot_that_made_products_is_progress() {
        use lifescape_planner::NodeKind;
        let coins = NodeKind::HaveCoins { amount: 100_000 };
        assert!(super::coin_lot_progress(&coins, "trade:craft/smith:steel_chainbody", 31));
        assert!(!super::coin_lot_progress(&coins, "trade:craft/smith:steel_chainbody", 0), "nothing made");
        assert!(!super::coin_lot_progress(&coins, "gather:mining/coal", 31), "no lot");
        assert!(!super::coin_lot_progress(&NodeKind::ObtainItems { item: "logs".to_string(), count: 1 }, "trade:craft/smith:steel_chainbody", 31));
    }

    /// Tour 2 P2-11: an engagement end, a success or a stop never echoes
    /// the node's older error; a failure prints it.
    #[test]
    fn the_fold_line_never_echoes_an_old_error() {
        let old = "protocol: unknown server opcode 149";
        assert_eq!(super::fresh_error(&Verdict::EngagementEnd, old), "");
        assert_eq!(super::fresh_error(&Verdict::Done, old), "");
        assert_eq!(super::fresh_error(&Verdict::Stopped, old), "");
        assert_eq!(super::fresh_error(&Verdict::Failure("x".to_string()), old), old);
        assert_eq!(super::fresh_error(&Verdict::Blocked(vec!["coal".to_string()]), "Blocked: missing coal"), "Blocked: missing coal");
    }

    #[test]
    fn xp_toward_reads_the_report_rows() {
        let report = lifescape_json::parse(
            r#"{"xp":[{"skill":"attack","stat":0,"xp":3492},{"skill":"prayer","stat":5,"xp":652},{"skill":"hitpoints","stat":3,"xp":1545}]}"#,
        )
        .expect("json");
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachLevel { skill: "prayer".to_string(), level: 40 }), 652);
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachLevel { skill: "magic".to_string(), level: 40 }), 0);
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::ReachCombat { level: 15 }), 3492 + 1545, "combat skills only");
        assert_eq!(super::xp_toward(&report, &lifescape_planner::NodeKind::MakeMoney), 0);
    }
}

#[cfg(test)]
#[path = "brain_sim_tests.rs"]
mod sim;
