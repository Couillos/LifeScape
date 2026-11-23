#![forbid(unsafe_code)]
//! The life upkeep and the graph state of the launch pass (lot 4 of the
//! launch refactor): each rule asks the planner's pure decision
//! (`lifescape_planner::upkeep*`), writes the change it returns into the
//! life, and performs what the decision asks of the world through
//! `UpkeepFx` (cancel a quote, free a board, draw a goal, print a line),
//! in the order the pass always had. Generic over the world: the daemon's
//! (`LiveUpkeepFx`) or the recording fake of the equivalence tests.
//!
//! What stays here by nature: the draws (`succeed_goal`, `redraw`,
//! `ensure`: they read the whole fleet and the market), the coverage log
//! (a per-account throttle table), the seats of a restart (a memory table).
//! The one-time D3 mark and A4 rebalance no longer run here (lot 5):
//! `migrate_cmd`.

use lifescape_planner::upkeep::{self, GiveBack};
use lifescape_planner::upkeep_ambition as ambition;
use lifescape_planner::upkeep_graph as graph;
use lifescape_planner::upkeep_role::{self as role, Assigned, OperatorStep, Switch};
use lifescape_planner::{GraphState, Life};

use crate::brain_launch_pass::{Flow, Save};
use crate::brain_upkeep_fx::UpkeepFx;

/// Life upkeep, first half: the operator's switch and the market maker.
/// True when the life is a quoting maker afterwards (no ambition pass, no
/// gear, tool or wish bid, no alchemy: its post, its quotes, its bank, Q6).
pub(crate) fn operator_and_maker<F: UpkeepFx>(fx: &F, life: &mut Life, dirty: &mut bool) -> bool {
    // An operator ambition switch filed by the CLI (`brain_operator`).
    if apply_pending(fx, life) {
        *dirty = true;
    }
    // Makers unused (user 2026-11-20): a maker life takes the neediest
    // trade, its quotes cancelled.
    if retire(fx, life) {
        *dirty = true;
    }
    // A market maker at its capital target leaves every other activity
    // for good and quotes its slice (plan marché Q6).
    if maybe_switch(fx, life) {
        *dirty = true;
    }
    lifescape_planner::quoting(life)
}

fn log_all<F: UpkeepFx>(fx: &F, lines: &[String]) {
    for line in lines {
        fx.log(line);
    }
}

/// Apply the switch filed for this life, if any, and lift the bans a
/// switch caused (ryan_mason 2026-11-18: its abandoned nodes read as a
/// stuck graph, magic banned 6000 ticks); true when the life changed.
pub(crate) fn apply_pending<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let lines = role::lift_role_bans(life, fx.clock());
    log_all(fx, &lines);
    let lifted = !lines.is_empty();
    let Some(kind) = fx.take_pending(&life.account) else { return lifted };
    match role::operator_step(life, &kind) {
        OperatorStep::Done(lines) => {
            log_all(fx, &lines);
            lifted
        }
        // A trade or `natural`: the ambition is given on a copy of the life
        // (it changes only when the step succeeded), the open work stops,
        // the next goal is drawn at once.
        OperatorStep::Assign { draft, switch } => {
            let mut draft = *draft;
            let given = match &switch {
                Switch::Natural => fx.assign_natural(&mut draft),
                Switch::Trade(kind) => fx.assign_kind(&mut draft, kind, &format!("operator:{kind}")),
            };
            match role::operator_assigned(life, draft, &switch, given, fx.clock().now) {
                Assigned::Refused(line) => {
                    fx.log(&line);
                    lifted
                }
                Assigned::Drawing(line) => {
                    fx.log(&line);
                    let label = fx.draw_next(life);
                    if let Some(line) = role::operator_drawn(life, &switch, label.as_deref()) {
                        fx.log(&line);
                    }
                    true
                }
            }
        }
        OperatorStep::Alcher => {
            let stamp = fx.stamp();
            let now = fx.clock().now;
            let mut switched = Err(String::new());
            fx.with_now(&mut |bot| switched = role::operator_alcher(life, bot, stamp, now));
            match switched {
                Ok(line) => {
                    fx.log(&line);
                    // The next goal at once (alchemy first), like the market
                    // maker's switch: never a stuck graph that bans its skills.
                    let label = fx.draw_next(life);
                    let line = role::alcher_drawn(life, label.as_deref());
                    fx.log(&line);
                    true
                }
                Err(line) => {
                    fx.log(&line);
                    lifted
                }
            }
        }
    }
}

/// The switch of a playing maker at its capital target: open buys and
/// listings of its slice withdrawn (reason `role`), every other node
/// abandoned, goal `MakeMarket`. True when the life changed.
pub(crate) fn maybe_switch<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let Some(role_held) = lifescape_planner::maker_role(life) else { return false };
    let coins = fx.coins();
    if !lifescape_planner::switch_due(&role_held, coins) {
        return false;
    }
    let slice = fx.slice_items(role_held.slice);
    if let Some(rows) = fx.orders() {
        for order in role::switch_cancels(&rows, &slice) {
            fx.cancel(order, crate::market_maker::ROLE_REASON);
        }
    }
    let clock = fx.clock();
    let line = role::apply_switch(life, &role_held, coins, clock);
    // Its bid answer, if any, ends with its old role: claims, purchases
    // and seats freed (review 2026-11-20).
    fx.free_board(&life.account);
    fx.log(&line);
    true
}

/// Retires a maker life while makers are unused: its resting quotes
/// cancelled (reason `maker_retired`), the producer archetype back, every
/// open node abandoned, the neediest trade drawn; its stock and coins stay
/// its own. True when the life changed.
pub(crate) fn retire<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    if !role::retire_due(life) {
        return false;
    }
    let mut cancelled = 0;
    if let Some(rows) = fx.orders() {
        for order in role::retire_cancels(&rows) {
            cancelled += usize::from(fx.cancel(order, crate::market_maker::RETIRED_REASON));
        }
    }
    let line = role::apply_retire(life, cancelled, fx.clock().now);
    fx.free_board(&life.account);
    fx.log(&line);
    fx.redraw_neediest(life, "market maker retired");
    lifescape_planner::normalize(life);
    true
}

/// Writes a give-back, draws the next goal, closes the give-back.
fn give_back<F: UpkeepFx>(fx: &F, life: &mut Life, give: &GiveBack) -> bool {
    let line = upkeep::apply_give_back(life, give, fx.clock().now);
    fx.log(&line);
    let label = fx.succeed_goal(life, false);
    let line = upkeep::finish_give_back(life, give, label.as_deref());
    fx.log(&line);
    true
}

/// A bid answer whose production every site refused past one cooldown is
/// given back, the ambition drawn again (2026-11-20).
pub(crate) fn release_blocked_bid<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    match upkeep::blocked_bid_give(life, fx.clock()) {
        Some(give) => give_back(fx, life, &give),
        None => false,
    }
}

/// An answer off the bot's trade (drawn before `bid_trade`) is given back,
/// the need-drawn ambition drawn again (2026-11-20).
pub(crate) fn release_off_trade<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    match upkeep::off_trade_give(life) {
        Some(give) => give_back(fx, life, &give),
        None => false,
    }
}

/// An answer whose level is above the bot's and its open milestones
/// (drawn while the ladder's top counted the training free) is given back,
/// the draw weighs it again (2026-11-20).
pub(crate) fn release_beyond_reach<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    match upkeep::beyond_reach_give(life, &|skill| fx.level_of(skill)) {
        Some(give) => give_back(fx, life, &give),
        None => false,
    }
}

/// A side demand goal whose value fell below the bot's time is given back
/// (2026-11-21: gem farms while the price crashed).
pub(crate) fn release_unpaying<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let Some(query) = upkeep::serve_query(life) else { return false };
    let facts = fx.serve_facts(life, &query);
    match upkeep::unpaying_give(life, &query, facts) {
        Some(give) => give_back(fx, life, &give),
        None => false,
    }
}

/// A demand milestone whose serving does not pay the bot's time is set
/// aside, persisted lives included (2026-11-21: gem farms of crafters).
pub(crate) fn set_aside_unpaying<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let Some(items) = upkeep::milestone_query(life) else { return false };
    let judged = fx.judge(life, &items);
    aside_judged(fx, life, &judged)
}

/// Sets aside the milestones judged unpaying (`judged`: the verdict of each
/// open demand milestone id) and gives back the goal of one.
pub(crate) fn aside_judged<F: UpkeepFx>(fx: &F, life: &mut Life, judged: &[(String, lifescape_planner::serve_value::ServeVerdict)]) -> bool {
    let Some(aside) = upkeep::milestone_aside(life, judged, fx.clock().now) else { return false };
    log_all(fx, &aside.lines);
    if let Some(give) = aside.give {
        give_back(fx, life, &give);
    }
    true
}

/// A changed ambition (review, coverage repair) replaces the goal the
/// earlier one drew; side, bid and alchemy goals run on (D3, 2026-11-19).
pub(crate) fn replace_stale_goal<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    match upkeep::stale_goal_give(life) {
        Some(give) => give_back(fx, life, &give),
        None => false,
    }
}

/// A revised catalogue ladder drops the stale quotas it no longer offers;
/// the goal of a dropped milestone closes done (no ban) and the next one
/// is drawn from the revised ladder.
pub(crate) fn sync_catalog<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let Some(drop) = ambition::drop_stale(life) else { return false };
    if let Some(line) = &drop.line {
        fx.log(line);
    }
    // The supply rule applies to the persisted quotas too (user rule,
    // 2026-11-20): a tool nobody can get, an idle quota eating no pile
    // while one waits.
    let surplus = fx.fleet_surplus();
    let mut synced = None;
    let mut pending = Some(drop);
    fx.with_now(&mut |bot| synced = pending.take().and_then(|drop| ambition::sync_supply(life, drop, &surplus, bot)));
    let Some(synced) = synced else { return false };
    log_all(fx, &synced.lines);
    if synced.close_running {
        let label = fx.succeed_goal(life, false);
        let line = ambition::catalog_drawn(life, label.as_deref());
        fx.log(&line);
    }
    true
}

/// Life upkeep, second half: ambition (ensure, coverage,
/// catalogue sync, stale goal), bid releases, unpaying
/// releases, stale goal work, re-engage after a restart. A quoting maker
/// has none of it.
pub(crate) fn ambition_upkeep<F: UpkeepFx>(fx: &F, life: &mut Life, dirty: &mut bool, quoting: bool) {
    // A quoting maker has none of it (every rule below was guarded `!quoting`).
    if quoting {
        return;
    }
    // Every life holds an ambition (2026-11-13): the first pass that
    // meets one without assigns the whole fleet (coverage first).
    if fx.timed("ambition.ensure", || fx.ensure(life)) {
        *dirty = true;
    }
    // Coverage log (A2, 2026-11-21): an uncovered group is logged; no
    // running ambition is retired to fill it.
    fx.timed("ambition.coverage", || fx.coverage(life));
    // The one-time D3 mark and A4 rebalance left the loop (lot 5, 2026-11-23):
    // `orchestrator migrate-lives` (`migrate_cmd`) applies them offline.
    // A revised catalogue ladder drops the stale quotas it no longer offers.
    if sync_catalog(fx, life) {
        *dirty = true;
    }
    // A changed ambition (review, coverage repair) replaces the goal the
    // earlier one drew; side, bid and alchemy goals run on.
    if replace_stale_goal(fx, life) {
        *dirty = true;
    }
    // A bid answer whose production every site refused past one cooldown
    // is given back, the ambition drawn again (2026-11-20).
    if release_blocked_bid(fx, life) {
        *dirty = true;
    }
    // An answer off the bot's trade (drawn before `bid_trade`) is given
    // back, the need-drawn ambition drawn again (2026-11-20).
    if release_off_trade(fx, life) {
        *dirty = true;
    }
    // An answer whose level is above the bot's and its open milestones
    // (drawn while the ladder's top counted the training free) is given
    // back, the draw weighs it again (2026-11-20).
    if release_beyond_reach(fx, life) {
        *dirty = true;
    }
    // A side demand goal whose value fell below the bot's time
    // is given back (2026-11-21: gem farms while the price crashed).
    if release_unpaying(fx, life) {
        *dirty = true;
    }
    // A demand milestone whose serving does not pay the bot's time is set
    // aside, persisted lives included (2026-11-21: gem farms of crafters).
    if set_aside_unpaying(fx, life) {
        *dirty = true;
    }
    // Open work of earlier goals nothing of the current goal waits on
    // leaves, persisted lives included (2026-11-22: 34 stale nodes).
    if let Some(line) = upkeep::stale_work_line(life) {
        fx.log(&line);
        *dirty = true;
    }
    // After a daemon restart, an open bid answer holds its seats again.
    fx.reengage(life);
}

/// The market rescue (user decision 2026-11-13): before a stuck goal bans
/// anything, an item on the failed path the market can sell is bought.
fn market_rescue<F: UpkeepFx>(fx: &F, life: &mut Life) -> bool {
    let now = fx.clock().now;
    graph::market_rescue(life, fx, now, &mut |line| fx.log(&line))
}

/// One stuck pass. True when the life changed (save it). A spell is noted
/// once, the goal closed once (done when the live levels meet it, stuck
/// otherwise: its skills banned), the draw retried as bans expire, the
/// cooldown revive, then the cap (rule 7: the operator decides).
fn on_stuck<F: UpkeepFx>(fx: &F, life: &mut Life, abandoned: &[String]) -> bool {
    match graph::spell(&life.history) {
        graph::Spell::Final => retry_draw(fx, life).1,
        graph::Spell::Fresh => {
            let opened = graph::open_spell(life, &|skill| fx.level_of(skill), abandoned, fx.clock().now);
            fx.log(&opened.line);
            let drawn = if opened.closed { fx.redraw(life).0 } else { fx.succeed_goal(life, opened.stuck) };
            let line = graph::spell_drawn(life, drawn.as_deref(), opened.stuck);
            fx.log(&line);
            true
        }
        graph::Spell::Waiting(since) => {
            let (drawn, changed) = retry_draw(fx, life);
            if drawn {
                return true;
            }
            match graph::revive(life, since, fx.clock()) {
                None => changed,
                Some(lines) => {
                    log_all(fx, &lines);
                    true
                }
            }
        }
    }
}

/// Retry the goal draw of a closed goal (bans may have expired). Returns
/// (a goal was drawn, the life changed).
fn retry_draw<F: UpkeepFx>(fx: &F, life: &mut Life) -> (bool, bool) {
    let (drawn, changed) = fx.redraw(life);
    let (drawn, changed, line) = graph::retry_drawn(life, drawn.as_deref(), changed);
    if let Some(line) = line {
        fx.log(&line);
    }
    (drawn, changed)
}

/// A reached goal over older goals' abandoned nodes is Finished, never a
/// stuck spell. `dirty`: the pass changed the life before.
pub(crate) fn graph_state<F: UpkeepFx>(fx: &F, life: &mut Life, dirty: bool) -> Flow {
    match fx.timed("stuck.effective_state", || graph::effective_state(life, &|skill| fx.level_of(skill))) {
        GraphState::Launchable => Flow::Continue,
        GraphState::Running(_) => Flow::Stop(Save::IfDirty),
        // A life never ends on a goal: done draws the next one. Stuck is
        // one spell: noted once, goal closed and redrawn, else the cooldown
        // revive, else the operator decides.
        GraphState::Stuck { abandoned } => {
            // Before any ban (user decision 2026-11-13): an item on the
            // failed path the market can sell is bought instead.
            if fx.timed("rescue.market", || market_rescue(fx, life)) {
                return Flow::Stop(Save::Always);
            }
            if fx.timed("stuck.on_stuck", || on_stuck(fx, life, &abandoned)) || dirty {
                return Flow::Stop(Save::Always);
            }
            Flow::Stop(Save::None)
        }
        // Closed once (live: `done:` was re-pushed every pass), then the
        // draw is retried every pass: a ban expiry alone brings the bot back.
        GraphState::Finished => {
            let closing = graph::finished_closing(life);
            if closing {
                fx.close_goal(life, false);
            }
            let (drawn, changed) = fx.timed("succession.redraw", || fx.redraw(life));
            if let Some(line) = graph::finished_drawn(life, drawn.as_deref(), closing) {
                fx.log(&line);
            }
            if closing || changed || dirty {
                return Flow::Stop(Save::Always);
            }
            Flow::Stop(Save::None)
        }
    }
}
