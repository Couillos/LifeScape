#![forbid(unsafe_code)]
//! Launch pass of the brain (split from `brain.rs`, rule 4): one idle
//! worker, one decision. Order: live snapshot (no decision without it),
//! invariants (`normalize`), the running node (market fill wait, lost
//! report), the tutorial gate, then the next ready node (resolve, map,
//! dispatch). Every status move goes through `lifescape_planner::lifecycle`.

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{draw_engagement_ticks, map_method, resolve_ready, Engagement, GraphState, Life, NodeKind, NodeStatus};

use crate::brain::{cleanup_keep, load_life, maybe_insert_cleanup, push_history, save_life};
use crate::brain_plan::{bank_types, carried_tools, holdings_count, live_combat, live_stacks, merge_keep, pick_flip_target, ready_ctx, worn_names};
use crate::brain_tutorial::{tutorial_gate, TutorialGate};
use crate::daemon::Orchestrator;
use crate::jobs::Job;

/// Typed launch failure (P3.6): the locked method names no executable
/// (unmapped prefix, unregistered script) or resolves to nothing. A
/// contract error: abandoned at once with its cause, never retried.
pub(crate) fn fail_no_script(life: &mut Life, position: usize, method: &str) {
    let id = life.nodes[position].id.clone();
    crate::logln!("brain {}: no script for {method}, abandoning {id}", life.account);
    lifescape_planner::record_contract_error(life, position, &format!("no-script: {method}"));
    push_history(life, &id, "abandoned");
}

fn job_idle(job: &Job) -> bool {
    let Ok(inner) = job.inner.try_lock() else {
        return false;
    };
    // Disconnected workers never take orders: the operator reopens them.
    // A stale script NAME never blocks: only `phase` tells whether a
    // script thread is alive (the worker itself refuses double `run`
    // with busy). Light heartbeats keep carrying the last script name
    // long after it finished; waiting on None would stall the brain.
    inner.phase == "idle" && !inner.held && !inner.paused && inner.status != "disconnected"
}

fn seconds_since_history(life: &Life, node: &str, outcome: &str) -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.iter().rev().find(|entry| entry.node == node && entry.outcome == outcome).map(|entry| now - entry.ended)
}

/// A `coins:N` need computed on the free wallet: the wallet to earn adds
/// back the coins the bot's other resting buys commit, so the
/// `have_coins` target it files really frees `N` (other needs unchanged).
pub(crate) fn lift_coin_need(need: &str, committed: i64) -> String {
    match need.strip_prefix("coins:").and_then(|amount| amount.parse::<i64>().ok()) {
        Some(amount) => format!("coins:{}", amount.saturating_add(committed.max(0))),
        None => need.to_string(),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

fn save_or_log(life: &Life) {
    if let Err(error) = save_life(life) {
        crate::logln!("{error}");
    }
}

/// Launch pass for one idle worker. Order: live snapshot (no decision
/// without it), invariants (`normalize`), the running node (market fill
/// wait, lost report), the tutorial gate, then the next ready node.
pub(crate) fn maybe_launch(orch: &Arc<Orchestrator>, job: &Arc<Job>) {
    if !job_idle(job) {
        return;
    }
    let Some(mut life) = load_life(&job.account) else { return };
    // Never decide on a missing snapshot (bt7ktb8c: an empty live read as
    // "not on the island" launched fishing there). A contended lock is
    // the same: next pass.
    let live = match job.inner.try_lock() {
        Ok(inner) => inner.live.clone(),
        Err(_) => return,
    };
    let Some(live) = live else { return };
    let mut dirty = false;
    // Every life holds an ambition (2026-11-13): the first pass that
    // meets one without assigns the whole fleet (coverage first).
    if crate::brain_ambition::ensure(orch, &mut life, &live) {
        dirty = true;
    }
    // Coverage repair (review 2026-11-13): an uncovered group is logged
    // and a redundant bot past the change throttle takes it.
    if crate::brain_ambition::coverage_pass(&mut life, &live) {
        dirty = true;
    }
    // Space pressure (L8) precedes the terminal check: a full bank needs
    // its janitor even when the graph is done.
    if maybe_insert_cleanup(&mut life, bank_types(job)) {
        let inserted = life.nodes.last().map(|node| node.id.clone()).unwrap_or_default();
        crate::logln!("brain {}: bank past {} types, inserting {}", job.account, lifescape_planner::BANK_TYPE_LIMIT, inserted);
        dirty = true;
    }
    for change in lifescape_planner::normalize(&mut life) {
        crate::logln!("brain {}: normalize {change}", job.account);
        dirty = true;
    }
    // The running node with an idle worker: either a market order
    // awaiting its fill (bounded), or a run whose report never came
    // (worker crash, daemon restart): counted as a failure, so a script
    // that kills its worker cannot relaunch forever.
    if let Some(active_id) = life.active.clone() {
        if recent_launch(&job.account) {
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        if let Some(position) = life.nodes.iter().position(|node| node.id == active_id) {
            let awaiting = seconds_since_history(&life, &active_id, "awaiting_fill");
            match (&life.nodes[position].kind.clone(), awaiting) {
                // A wait locked by an older daemon (or a worker-side buy)
                // parks: the bid rests, the worker works on (no idle bot).
                (NodeKind::ObtainItems { item, count }, Some(waited)) => {
                    if crate::brain_await::settle_fill(job, &mut life, position, item, *count, waited) {
                        crate::logln!("brain {}: {active_id} parked on the book, the worker works on", job.account);
                        // Its last entry stays `awaiting_fill`: the deadline
                        // keeps counting from the placement.
                        lifescape_planner::park_for_fill(&mut life, position);
                    }
                }
                // Right after a daemon restart the restored workers read
                // idle before their scripts report: an operator restart,
                // never a crash (live 2026-11-12: every restart cost each
                // active node one failure). Same for an operator `unload`,
                // `close` or `open` (redeploy). Retry uncounted, like `stopped`.
                _ if crate::brain_grace::take_operator_stop(&job.account) || crate::brain_grace::first_idle_since_start(&job.account) => {
                    crate::logln!("brain {}: {active_id} idle after a daemon restart or an operator stop, retrying uncounted", job.account);
                    lifescape_planner::retry_uncounted(&mut life, position);
                    push_history(&mut life, &active_id, "stopped");
                }
                _ => {
                    let text = "no report: worker went idle without reporting (crash or restart)";
                    crate::logln!("brain {}: {active_id} {text}", job.account);
                    let fold = lifescape_planner::record_failure(&mut life, position, text);
                    push_history(&mut life, &active_id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
                }
            }
            lifescape_planner::normalize(&mut life);
            dirty = true;
        }
    }
    // Tutorial gate: on the island only the tutorial runs; no position
    // yet means no launch at all.
    match tutorial_gate(&mut life, &live) {
        TutorialGate::Launch(at) => {
            if !crate::registry::script_known("tutorial") {
                fail_no_script(&mut life, at, "tutorial");
                save_or_log(&life);
                return;
            }
            let id = life.nodes[at].id.clone();
            crate::logln!("brain {}: launching tutorial --node {id}", job.account);
            lifescape_planner::mark_launched(&mut life, at);
            push_history(&mut life, &id, "launched");
            note_launch(&job.account);
            save_or_log(&life);
            let args = vec![lifescape_json::str_value("--node"), lifescape_json::str_value(&id)];
            let reply = orch.run_job(&job.account, "tutorial", &args);
            if reply.get("ok").as_bool() != Some(true) {
                crate::logln!("brain {}: launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
            }
            return;
        }
        TutorialGate::Hold(reason) => {
            if life.history.last().is_none_or(|entry| entry.outcome != "hold") {
                crate::logln!("brain {}: holding launches: {reason}", job.account);
            }
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        TutorialGate::Closed(id) => {
            crate::logln!("brain {}: seen off the island, closing tutorial {id}", job.account);
            push_history(&mut life, &id, "tutorial_done");
            lifescape_planner::normalize(&mut life);
            dirty = true;
        }
        TutorialGate::Clear => {}
    }
    // Graph tasks read the pack (tools, coins): none before it streamed.
    // The tutorial above needs only the position (the island hides the
    // pack until its own tab opens).
    if live.get("inv_streamed").as_bool() != Some(true) {
        if dirty {
            save_or_log(&life);
        }
        return;
    }
    // PRD3 lot 3: idle bots list their tradeable surplus on the market
    // (rate-limited, never an NPC sale).
    crate::brain_sales::maybe_list_surplus(job, &life);
    // PRD3 lot 5: upgrade wishes bid on the market, never blocking.
    crate::brain_wishlist::maybe_bid_wishes(job, &life);
    // Equipment ladder (2026-11-13): the next tier as a graph step, or a
    // standing bid that waits for a producer.
    if crate::brain_gear::gear_pass(job, &mut life, &live) {
        dirty = true;
    }
    // Market buys parked on the book settle every pass; the worker never
    // waits for them (no idle bot, 2026-11-13).
    let (settled, waiting) = crate::brain_filler::settle_parked(job, &mut life);
    if settled {
        dirty = true;
    }
    // A reached goal over older goals' abandoned nodes is Finished, never
    // a stuck spell (`brain_stuck::effective_state`).
    match crate::brain_stuck::effective_state(&life, &live) {
        GraphState::Launchable => {}
        GraphState::Running(_) => {
            if dirty {
                save_or_log(&life);
            }
            return;
        }
        // A life never ends on a goal: done draws the next one. Stuck is
        // one spell (`brain_stuck`): noted once, goal closed and redrawn,
        // else the cooldown revive, else the operator decides.
        GraphState::Stuck { abandoned } => {
            // Before any ban (user decision 2026-11-13): an item on the
            // failed path the market can sell is bought instead.
            if crate::brain_rescue::market_rescue(job, &mut life, &live) {
                save_or_log(&life);
                return;
            }
            if crate::brain_stuck::on_stuck(&mut life, &live, &abandoned) || dirty {
                save_or_log(&life);
            }
            return;
        }
        // Closed once (live: `done:` was re-pushed every pass), then the
        // draw is retried every pass: a ban expiry alone brings the bot back.
        GraphState::Finished => {
            let closing = !lifescape_planner::goal_closed(&life.history);
            if closing {
                crate::brain_succession::close_goal(&mut life, &live, false);
            }
            let (drawn, changed) = crate::brain_succession::redraw(&mut life, &live);
            match drawn {
                Some(label) => {
                    crate::logln!("brain {}: goal done -> next goal {label}", job.account);
                    lifescape_planner::normalize(&mut life);
                }
                None if closing => crate::logln!(
                    "brain {}: no goal left to draw (see the reason above), retrying every pass",
                    job.account
                ),
                None => {}
            }
            if closing || changed || dirty {
                save_or_log(&life);
            }
            return;
        }
    }
    // The gear sub-goal first, then the goal's next node; the tutorial
    // only ever starts through its gate. Parked market waits are skipped:
    // with nothing else ready, a short money window fills the wait.
    let Some(position) = lifescape_planner::next_ready_except(&life, &waiting) else {
        if !waiting.is_empty() && crate::brain_filler::launch_filler(orch, job, &mut life, &live, &waiting) {
            return;
        }
        if dirty {
            save_or_log(&life);
        }
        return;
    };
    // A climb the live levels already meet closes without a launch (live
    // 2026-11-13: b8jxrdd2 ran "attack 10" at attack 45 for 18 015 ticks).
    let level_of = |skill: &str| crate::brain_succession::live_level(&live, skill);
    if let Some(reached) = lifescape_planner::reached_live(&life.nodes[position], &level_of, live_combat(&live)) {
        let id = life.nodes[position].id.clone();
        crate::logln!("brain {}: {id} {:?} already reached (live {reached}), done without launch", job.account, life.nodes[position].kind);
        lifescape_planner::complete_node(&mut life, position, i64::from(reached));
        push_history(&mut life, &id, "already_reached");
        lifescape_planner::normalize(&mut life);
        save_or_log(&life);
        return;
    }
    // Quests the bot finished (engine save + done quest nodes): a quest
    // node already complete closes, a method behind a missing quest never
    // launches (the resolver plans the quest first).
    let quests_done = crate::brain_rescue::quests_done(&life);
    if crate::brain_rescue::close_done_quest(&mut life, position, &quests_done) {
        save_or_log(&life);
        return;
    }
    let mut ctx = {
        let node = life.nodes[position].clone();
        ready_ctx(&life, &node)
    };
    ctx.quests_done = quests_done;
    // Own wallet gates the market leg (same coverage rule as the
    // carnet): a broke bot farms instead of placing doomed orders.
    // Carried tools gate the gather legs the same way (t04: rod-less
    // sardines never dispatch).
    ctx.coins = holdings_count(job, "coins");
    // A purchase bids on the FREE wallet (review 2026-11-13): coins minus
    // what this bot's other resting buys (gear step, tool wishes) commit.
    let mut committed = 0;
    if let NodeKind::ObtainItems { item, .. } = &life.nodes[position].kind {
        ctx.held = holdings_count(job, item);
        committed = crate::brain_gear::committed_coins(&job.account, crate::brain_gear::obj_id(item));
        ctx.coins = (ctx.coins - committed).max(0);
    }
    ctx.tools = carried_tools(job);
    ctx.bank_known = live.get("bank_streamed").as_bool() == Some(true);
    // The brain picks the site: the bot's combat level and position.
    ctx.combat = live_combat(&live);
    ctx.position = live.get("position").as_arr().and_then(|cells| {
        let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
        Some((part(1)?, part(2)?, part(0)?))
    });
    crate::brain_market::fill_ctx(&mut ctx, &life.nodes[position], &live);
    // An obtain the holdings already meet closes without a launch
    // (positive proof: pack, bank, worn), like a met coin target below.
    if let NodeKind::ObtainItems { item, count } = &life.nodes[position].kind {
        if ctx.held >= *count {
            let (id, item) = (life.nodes[position].id.clone(), item.clone());
            crate::brain_market::cancel_open_buys(&job.account, &item);
            lifescape_planner::complete_node(&mut life, position, ctx.held);
            push_history(&mut life, &id, "already_held");
            crate::logln!("brain {}: {id} holds {} {item}, done without launch", job.account, ctx.held);
            lifescape_planner::normalize(&mut life);
            save_or_log(&life);
            return;
        }
    }
    // A ladder purchase spends its wallet share only; with no seller
    // left and a wallet that can bid, the step withdraws and the standing
    // bid waits for a producer (never a worker held on an empty book).
    if lifescape_planner::gear_ladder::is_gear_obtain(&life, position) {
        ctx.coin_share_pct = Some(lifescape_planner::GEAR_SHARE_PCT);
        if lifescape_planner::gear_ladder::withdraw_due(&ctx) {
            let removed = lifescape_planner::withdraw_gear_step(&mut life, position);
            crate::logln!("brain {}: gear step {} withdrawn: no seller, the standing bid waits", job.account, removed.join(", "));
            for id in &removed {
                push_history(&mut life, id, "gear_withdrawn");
            }
            lifescape_planner::normalize(&mut life);
            save_or_log(&life);
            return;
        }
    }
    // A coin target already met closes without a launch (PRD3 lot 4).
    if let NodeKind::HaveCoins { amount } = life.nodes[position].kind {
        if ctx.coins >= amount {
            let id = life.nodes[position].id.clone();
            lifescape_planner::complete_node(&mut life, position, ctx.coins);
            push_history(&mut life, &id, "goal_reached");
            crate::logln!("brain {}: {id} holds {} coins >= {amount}, done", job.account, ctx.coins);
            lifescape_planner::normalize(&mut life);
            save_or_log(&life);
            return;
        }
    }
    {
        let node = &mut life.nodes[position];
        resolve_ready(node, &ctx);
        crate::logln!("brain {}: resolve {} -> {:?} (ctx coins={})", job.account, node.id, node.method, ctx.coins);
    }
    // A money or coin node never works a banned skill (D-SIDE: a side
    // money goal, a coin target): the next ranked method off the bans.
    if matches!(life.nodes[position].kind, NodeKind::MakeMoney | NodeKind::HaveCoins { .. }) {
        let banned = lifescape_planner::banned_skills(&life.history, &|stamp: i64| crate::brain_stuck::seconds_to_ticks(now_unix() - stamp));
        let money_id = life.nodes[position].id.clone();
        match lifescape_planner::steer_off_bans(&mut life.nodes[position], &ctx, &banned) {
            Ok(true) => crate::logln!("brain {}: {money_id} steered off banned {} -> {:?}", job.account, banned.join(","), life.nodes[position].method),
            Ok(false) => {}
            Err(reason) => {
                crate::logln!("brain {}: {money_id} waits: {reason}", job.account);
                lifescape_planner::retry_uncounted(&mut life, position);
                save_or_log(&life);
                return;
            }
        }
    }
    // The planner blocks typed when no method is workable (tool to
    // fund): the block becomes an obtain sub-task now, never a node that
    // flips Ready/Blocked forever without launching.
    if let NodeStatus::Blocked { reason } = life.nodes[position].status.clone() {
        let missing: Vec<String> = reason.strip_prefix("missing:").map(|need| vec![lift_coin_need(need, committed)]).unwrap_or_default();
        let id = life.nodes[position].id.clone();
        if missing.is_empty() {
            lifescape_planner::record_contract_error(&mut life, position, &format!("resolve blocked: {reason}"));
            push_history(&mut life, &id, "abandoned");
        } else {
            let level_of = |skill: &str| crate::brain_succession::live_level(&live, skill);
            let fold = lifescape_planner::record_block_from(&mut life, position, &missing, &level_of);
            push_history(&mut life, &id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "blocked" });
        }
        crate::logln!("brain {}: {id} blocked at resolve ({reason}) -> {}", job.account, life.nodes[position].status.as_str());
        lifescape_planner::normalize(&mut life);
        save_or_log(&life);
        return;
    }
    let method = life.nodes[position].method.clone().unwrap_or_default();
    if method.is_empty() {
        let id = life.nodes[position].id.clone();
        crate::logln!("brain {}: no method resolvable for {id}, abandoning", job.account);
        lifescape_planner::record_contract_error(&mut life, position, "no method resolvable (no workable source for this node)");
        push_history(&mut life, &id, "abandoned");
        lifescape_planner::normalize(&mut life);
        save_or_log(&life);
        return;
    }
    // Combat requirements are graph dependencies now: the site pick in
    // `resolve_ready` blocked typed (`missing:combat:N`) above, and the
    // block filed a `reach_combat` node the node depends on.
    let launch = position;
    let method = life.nodes[launch].method.clone().unwrap_or_default();
    // A redirect target without a method never fails typed on the empty
    // string: wait a pass (the gate re-carries it, then it launches).
    if method.is_empty() {
        crate::logln!("brain {}: redirect {} has no method yet, waiting", job.account, life.nodes[launch].id);
        let _ = save_life(&life);
        return;
    }
    // Iron never trades (D15): a stale market lock from an old file resets
    // instead of launching; the planner never locks one on fresh resolves.
    if !lifescape_planner::launch_allowed(&method, life.profile.ironman) {
        crate::logln!("brain {}: refusing {method} for Ironman", job.account);
        lifescape_planner::retry_uncounted(&mut life, launch);
        let _ = save_life(&life);
        return;
    }
    // Engagement: money draws [6000, 18000] at the activity start;
    // reach boxes at one window and retries; farm obtains work one
    // window then recheck holdings; cleanup works one window per pass.
    let budget: Option<(u64, u64)> = match &life.nodes[launch].kind {
        NodeKind::MakeMoney => {
            let chosen = draw_engagement_ticks(life.seed, life.history.len() as u64);
            life.engagement = Some(Engagement {
                node: life.nodes[launch].id.clone(),
                min_ticks: 6000,
                max_ticks: chosen.max(6000),
                chosen: chosen.max(6000),
            });
            let max = life.engagement.as_ref().map(|engaged| engaged.max_ticks).unwrap_or(6000);
            Some((6000, max))
        }
        NodeKind::ReachLevel { .. } | NodeKind::ReachCombat { .. } => Some((0, 18000)),
        // Every production leg of an obtain works one window, then the
        // holdings recheck (PRD3: smith, spin, mill… not only farm).
        NodeKind::ObtainItems { .. } if !method.starts_with("market:") && !method.starts_with("shop:") => Some((6000, 6000)),
        NodeKind::HaveCoins { .. } => Some((6000, 6000)),
        NodeKind::SpaceCleanup => Some((0, 6000)),
        _ => None,
    };
    // The held units ride as the obtain's progress: the launch asks only
    // what is still missing (`--want <item> N`, shopbuy, sell legs).
    if matches!(life.nodes[launch].kind, NodeKind::ObtainItems { .. }) {
        lifescape_planner::record_progress(&mut life, launch, ctx.held);
    }
    let node_snapshot = life.nodes[launch].clone();
    // A market buy needs no worker (PRD3 lot 4): the brain places the
    // bid on the daemon market and the node waits for the fill.
    if method == "market:buy" {
        let id = node_snapshot.id.clone();
        match crate::brain_market::place_obtain_bid(job, &node_snapshot, &ctx) {
            Ok(_) => {
                // Parked, not locked (no idle bot, 2026-11-13): the bid
                // rests while the worker runs the next node or a filler.
                life.nodes[launch].progress_current = ctx.held;
                lifescape_planner::park_for_fill(&mut life, launch);
                push_history(&mut life, &id, "awaiting_fill");
            }
            Err(error) => {
                let fold = lifescape_planner::record_failure(&mut life, launch, &format!("market bid refused: {error}"));
                push_history(&mut life, &id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
            }
        }
        save_or_log(&life);
        return;
    }
    let Some((script, args)) = map_method(&method, &node_snapshot, budget) else {
        // No executable for this method: typed failure, never a silent
        // spin — the dashboard shows it and the 3-strike rule can
        // abandon it (replan on new scripts, never ghost-retry).
        fail_no_script(&mut life, launch, &method);
        let _ = save_life(&life);
        return;
    };
    let mut args = args;
    if matches!(node_snapshot.kind, NodeKind::ObtainItems { .. }) && !method.starts_with("shop:") {
        crate::brain_market::place_standing_bid(job, &node_snapshot, &ctx);
    }
    // D52: flip and cleanup never sell worn, work tools or graph items.
    // The keep list carries the tools the bot works with
    // (`brain_tools::kept_tools`), so the merchant never flips its net;
    // other tools are surplus (tools sell, 2026-11-13).
    // The flip leaves auto-pick only with no safe target (sans objet).
    if method == "market:flip" {
        let stacks = live_stacks(job);
        let mut keep = cleanup_keep(&life);
        keep.extend(crate::brain_tools::kept_tools(&life, &stacks));
        let worn = worn_names(job);
        if let Some((item, qty)) = pick_flip_target(&stacks, &keep, &worn) {
            crate::logln!("brain {}: flip D52 target {item} x{qty}", job.account);
            args.push(item);
            args.push(qty.min(100).max(1).to_string());
        } else {
            crate::logln!("brain {}: flip D52 no safe stock, clock ends it", job.account);
        }
    }
    // Cleanup carries its keep-list (graph obtains, quest checks) and the
    // Iron flag (D2 direct, no carnet) to the worker; coins, food and best
    // tools resolve worker-side.
    if method == "space:cleanup" {
        for kept in merge_keep(cleanup_keep(&life), worn_names(job)) {
            args.push("--keep".to_string());
            args.push(kept);
        }
        if life.profile.ironman {
            args.push("--iron".to_string());
        }
    }
    if !crate::registry::script_known(&script) {
        // The method is wired but its P3 script has not landed yet:
        // same typed failure, the script name rides in the error.
        fail_no_script(&mut life, launch, &format!("{method} -> {script}"));
        let _ = save_life(&life);
        return;
    }
    let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
    crate::logln!("brain {}: launching {script} {}", job.account, args.join(" "));
    // Queued, not run: the launcher thread owns the blocking worker RPC.
    // Mark Active now so the next pass does not double-queue; a refused
    // launch re-queues via crash recovery (no report ever arrives).
    lifescape_planner::mark_launched(&mut life, launch);
    let launched_id = life.nodes[launch].id.clone();
    push_history(&mut life, &launched_id, "launched");
    note_launch(&job.account);
    // Saved before the blocking RPC: a report racing the reply must find
    // the node active.
    save_or_log(&life);
    // Blocking worker RPC is fine here: this is the brain thread, not
    // the supervisor. A refused launch re-queues via crash recovery.
    let reply = orch.run_job(&job.account, &script, &values);
    if reply.get("ok").as_bool() != Some(true) {
        crate::logln!("brain {}: launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
    }
}

/// Launch grace table (30 s per account, memory only): stops crash
/// recovery from re-queueing while the worker is still picking up.
/// A `HashMap` in a `OnceLock`, never persisted, never shared.
fn launch_marks() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static LAUNCHED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
        std::sync::OnceLock::new();
    LAUNCHED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub(crate) fn recent_launch(account: &str) -> bool {
    launch_marks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(account)
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(30))
}

pub(crate) fn note_launch(account: &str) {
    // A stop noted before this launch (reopen of an idle bot) covered an
    // earlier run: it never excuses this one.
    crate::brain_grace::take_operator_stop(account);
    launch_marks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(account.to_string(), std::time::Instant::now());
}

#[cfg(test)]
mod tests {
    use super::lift_coin_need;

    /// Review 2026-11-13 (finding 6): a graph buy bids on the coins the
    /// bot's standing bids leave free; the wallet it must earn adds those
    /// commitments back, or the `have_coins` target is met while the
    /// purchase still cannot be afforded.
    #[test]
    fn a_coin_need_adds_the_committed_coins() {
        assert_eq!(lift_coin_need("coins:120", 40), "coins:160");
        assert_eq!(lift_coin_need("coins:120", 0), "coins:120");
        assert_eq!(lift_coin_need("tool:pickaxe", 40), "tool:pickaxe");
        assert_eq!(lift_coin_need("coins:many", 40), "coins:many");
    }
}
