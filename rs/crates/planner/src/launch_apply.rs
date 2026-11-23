#![forbid(unsafe_code)]
//! The life changes of a launch decision (lot 3 of the launch refactor):
//! `apply_to_life` writes into the life what `launch_decide::decide` chose
//! (complete a node, file a block, park a buy, mark a launch, the history
//! entries), and returns the log lines and effects that follow. No daemon,
//! no I/O: the same functions the brain always called, in the same order.

use crate::launch_decide::{AbandonKind, BlockStage, Decided, Emit, LaunchDecision};
use crate::launch_inputs::LaunchInputs;
use crate::plan_read::PlanRead;
use crate::{FailureFold, HistoryEntry, Life, Profile};

/// The planner's readers for one bot (`brain_plan_read::with_plan_read` on
/// the orchestrator side, the game prior in a test).
pub trait PlanSource {
    fn with_read(&self, account: &str, profile: &Profile, quests_done: &[String], body: &mut dyn FnMut(&PlanRead));
}

/// What applying a decision reads besides the life.
pub struct ApplyEnv<'a> {
    pub inputs: &'a LaunchInputs,
    /// F2P quests the bot finished (the readers judge a training guide on them).
    pub quests_done: &'a [String],
    pub reader: &'a dyn PlanSource,
}

/// One history entry, stamped `now`.
pub fn push_history(life: &mut Life, node: &str, outcome: &str, now: i64) {
    life.history.push(HistoryEntry { node: node.to_string(), started: now, ended: now, outcome: outcome.to_string() });
}

/// History outcome of a block fold: `cycle_broken` when the block would
/// have closed a dependency cycle (`planner::lifecycle_cycle`).
pub fn block_outcome(fold: &FailureFold) -> &'static str {
    match fold {
        FailureFold::Abandoned => "abandoned",
        FailureFold::CycleBroken(_) => "cycle_broken",
        FailureFold::Retry => "blocked",
    }
}

fn status_of(life: &Life, id: &str) -> String {
    // A broken cycle may have dropped the node: read it by id.
    life.nodes.iter().find(|node| node.id == id).map_or_else(|| "dropped".to_string(), |node| node.status.as_str())
}

/// The resolve prelude and the dispatch plan, written into the life: the
/// resolved node, the memory the pick and the lot changed, the engagement
/// and the progress of an obtain. Idempotent (the caller may apply it
/// early, before an effect that reads the life, and again with the outcome).
pub fn apply_pre(life: &mut Life, decided: &Decided) {
    if let Some(boundary) = &decided.boundary {
        life.nodes[decided.position] = boundary.node.clone();
        if let Some(memory) = &boundary.memory {
            life.memory = memory.clone();
        }
    }
    if let Some(plan) = &decided.plan {
        if let Some(engagement) = &plan.engagement {
            life.engagement = Some(engagement.clone());
        }
        if let Some(held) = plan.progress {
            crate::record_progress(life, decided.position, held);
        }
    }
}

/// Applies the decision to the life; returns what follows it (logs and
/// effects, in order). A decision that only asks (`Need`, `Withdraw*`)
/// changes nothing but the idempotent prelude.
pub fn apply_to_life(life: &mut Life, decided: &Decided, env: &ApplyEnv<'_>) -> Vec<Emit> {
    apply_pre(life, decided);
    let position = decided.position;
    let account = life.account.clone();
    let now = env.inputs.now;
    let mut out = Vec::new();
    match &decided.decision {
        LaunchDecision::Need(_) | LaunchDecision::WithdrawOwnListings { .. } | LaunchDecision::WithdrawInputListings { .. } | LaunchDecision::PlaceMarketBuy { .. } => {}
        LaunchDecision::AlreadyHeld { item, held } => {
            let id = life.nodes[position].id.clone();
            out.push(Emit::CancelOpenBuys(item.clone()));
            crate::complete_node(life, position, *held);
            push_history(life, &id, "already_held", now);
            out.push(Emit::Log(format!("brain {account}: {id} holds {held} {item}, done without launch")));
            crate::normalize(life);
        }
        LaunchDecision::GearStepWithdrawn { item } => {
            if let Some(item) = item {
                out.push(Emit::NoteGearWithdrawn(item.clone()));
            }
            let removed = crate::withdraw_gear_step(life, position);
            out.push(Emit::Log(format!("brain {account}: gear step {} withdrawn: no seller, the standing bid waits", removed.join(", "))));
            for id in &removed {
                push_history(life, id, "gear_withdrawn", now);
            }
            crate::normalize(life);
        }
        LaunchDecision::CoinTargetMet { amount, coins } => {
            let id = life.nodes[position].id.clone();
            crate::complete_node(life, position, *coins);
            push_history(life, &id, "goal_reached", now);
            out.push(Emit::Log(format!("brain {account}: {id} holds {coins} coins >= {amount}, done")));
            crate::normalize(life);
        }
        LaunchDecision::CoinCycle { regress } => {
            let id = life.nodes[position].id.clone();
            if let FailureFold::CycleBroken(line) = crate::lifecycle_cycle::break_cycle(life, position, "coins", regress) {
                out.push(Emit::Log(format!("brain {account}: {line}")));
                push_history(life, &id, "cycle_broken", now);
            }
            crate::normalize(life);
        }
        LaunchDecision::WaitUncounted { .. } => crate::retry_uncounted(life, position),
        LaunchDecision::CooledWait { reason } => on_resolve_wait(life, position, reason, env.inputs, &mut out),
        LaunchDecision::Abandon(AbandonKind::ResolveBlocked(reason)) => {
            let id = life.nodes[position].id.clone();
            crate::record_contract_error(life, position, &format!("resolve blocked: {reason}"));
            push_history(life, &id, "abandoned", now);
            let status = status_of(life, &id);
            out.push(Emit::Log(format!("brain {account}: {id} blocked at resolve ({reason}) -> {status}")));
            crate::normalize(life);
        }
        LaunchDecision::Abandon(AbandonKind::NoMethod) => {
            let id = life.nodes[position].id.clone();
            out.push(Emit::Log(format!("brain {account}: no method resolvable for {id}, abandoning")));
            crate::record_contract_error(life, position, "no method resolvable (no workable source for this node)");
            push_history(life, &id, "abandoned", now);
            crate::normalize(life);
        }
        LaunchDecision::Blocked { stage, missing } => {
            let id = life.nodes[position].id.clone();
            let level_of = |skill: &str| env.inputs.level_of(skill);
            let profile = life.profile.clone();
            let mut fold = None;
            env.reader.with_read(&account, &profile, env.quests_done, &mut |read| fold = Some(crate::record_wait_block(life, position, missing, &level_of, read)));
            let fold = fold.unwrap_or(FailureFold::Retry);
            push_history(life, &id, block_outcome(&fold), now);
            if let FailureFold::CycleBroken(line) = &fold {
                out.push(Emit::Log(format!("brain {account}: {line}")));
            }
            let status = status_of(life, &id);
            out.push(Emit::Log(match stage {
                BlockStage::Resolve(reason) => format!("brain {account}: {id} blocked at resolve ({reason}) -> {status}"),
                BlockStage::Inputs => format!("brain {account}: {id} inputs short before launch ({}) -> {status}", missing.join(", ")),
                BlockStage::Food => format!("brain {account}: {id} foodless before launch ({}) -> {status}", missing.join(", ")),
            }));
            crate::normalize(life);
        }
        LaunchDecision::RefuseIron { .. } => crate::retry_uncounted(life, position),
        LaunchDecision::NoScript { method, script } => {
            let id = life.nodes[position].id.clone();
            let wired = match script {
                Some(script) => format!("{method} -> {script}"),
                None => method.clone(),
            };
            out.push(Emit::Log(format!("brain {account}: no script for {wired}, abandoning {id}")));
            // Nothing of a lot it locked is on its way any more.
            out.push(Emit::ReleaseRun(id.clone()));
            crate::record_contract_error(life, position, &format!("no-script: {wired}"));
            push_history(life, &id, "abandoned", now);
        }
        LaunchDecision::RunScript { .. } => {
            crate::mark_launched(life, position);
            let id = life.nodes[position].id.clone();
            push_history(life, &id, "launched", now);
        }
    }
    out
}

/// The market buy came back: parked for its fill, or the failure counted.
pub fn settle_market_buy(life: &mut Life, position: usize, held: i64, placed: Result<(), String>, now: i64) {
    let id = life.nodes[position].id.clone();
    match placed {
        Ok(()) => {
            // Parked, not locked (no idle bot, 2026-11-13): the bid
            // rests while the worker runs the next node or a filler.
            life.nodes[position].progress_current = held;
            crate::park_for_fill(life, position);
            push_history(life, &id, "awaiting_fill", now);
        }
        Err(error) => {
            let fold = crate::record_failure(life, position, &format!("market bid refused: {error}"));
            push_history(life, &id, if fold == FailureFold::Abandoned { "abandoned" } else { "failed" }, now);
        }
    }
}

/// A resolve block every site of every candidate refused (`full:` /
/// `cooled:<method>`): never an abandonment. The method not cooled yet
/// cools for this bot and the node resolves again at once (its next
/// method, D6); every candidate cooled, the node stays open and waits out
/// the cooldown.
pub fn on_resolve_wait(life: &mut Life, index: usize, reason: &str, inputs: &LaunchInputs, out: &mut Vec<Emit>) {
    use crate::cooldown::{cool_entry, cooled_keys, resolve_wait, resolve_wait_step, ResolveWait, COOL_NODE, COOL_TICKS, WAIT_TAG};
    let Some(method) = resolve_wait(reason).map(str::to_string) else { return };
    let id = life.nodes[index].id.clone();
    let active = cooled_keys(&life.history, &|stamp: i64| inputs.ticks_of(inputs.now - stamp));
    let account = life.account.clone();
    match resolve_wait_step(&active, &method) {
        ResolveWait::CoolAndRetry => {
            out.push(Emit::Log(format!("brain {account}: {method} cooled {COOL_TICKS} ticks for this bot: {reason} ({id} stays open, next method)")));
            push_history(life, COOL_NODE, &cool_entry(&method, COOL_TICKS, reason), inputs.now);
        }
        ResolveWait::Wait => {
            out.push(Emit::Log(format!("brain {account}: {id} waits: {reason} (every method cooled, node stays open)")));
            push_history(life, &id, &format!("{WAIT_TAG}{method}"), inputs.now);
        }
    }
    crate::wait_open(life, index, reason);
}
