#![forbid(unsafe_code)]
//! Launch pass of the brain (split from `brain.rs`, rule 4): one idle
//! worker, one decision. Order: live snapshot (no decision without it),
//! invariants (`normalize`), the running node (market fill wait, lost
//! report), the tutorial gate, then the next ready node (resolve, map,
//! dispatch). Every status move goes through `lifescape_planner::lifecycle`.
//! The steps live in `brain_launch_steps` (pass level), `brain_launch_node`
//! (pick and context); the decision is the planner's (`launch_decide`),
//! driven by `brain_launch_drive` over the effects of `brain_launch_fx`;
//! the inputs in `brain_launch_inputs`.

use std::sync::Arc;

use lifescape_planner::Life;

use crate::brain::{load_life, push_history, save_life};
use crate::brain_launch_pass::{Flow, LaunchPass, Picked};
use crate::fleet::Fleet;
use crate::jobs::Job;
use crate::{brain_launch_drive as drive, brain_launch_node as node, brain_launch_steps as steps};

/// Typed launch failure (P3.6): the locked method names no executable
/// (unmapped prefix, unregistered script) or resolves to nothing. A
/// contract error: abandoned at once with its cause, never retried.
pub(crate) fn fail_no_script(life: &mut Life, position: usize, method: &str) {
    let id = life.nodes[position].id.clone();
    crate::logln!("brain {}: no script for {method}, abandoning {id}", life.account);
    // Nothing of a lot it locked is on its way any more.
    crate::market_claims::release_run(&life.account, &id);
    lifescape_planner::record_contract_error(life, position, &format!("no-script: {method}"));
    push_history(life, &id, "abandoned");
}

pub(crate) fn job_idle(job: &Job) -> bool {
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

/// Seconds the node has waited since its last `outcome` entry, the
/// daemon's downtime not counted (`downtime::awake_secs`): a buy placed
/// before a stop resumes its countdown at the restart.
pub(crate) fn seconds_since_history(life: &Life, node: &str, outcome: &str) -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    life.history.iter().rev().find(|entry| entry.node == node && entry.outcome == outcome).map(|entry| crate::downtime::awake_secs(entry.ended, now))
}

/// History outcome of a block fold (`launch_apply::block_outcome`).
pub(crate) fn block_outcome(fold: &lifescape_planner::FailureFold) -> &'static str {
    lifescape_planner::launch_apply::block_outcome(fold)
}

pub(crate) fn save_or_log(life: &Life) {
    if let Err(error) = save_life(life) {
        crate::logln!("{error}");
    }
}


/// Pass-level step: reads and changes the pass, goes on or stops it.
type Step = fn(&mut LaunchPass) -> Flow;

/// The pass before a node is picked, in order: life upkeep, bank space
/// and NPC sale, the running node, the tutorial gate, the pack guard, the
/// side market passes, the waits, the graph state.
const PASS_STEPS: [Step; 9] = [
    steps::operator_and_maker,
    steps::ambition_upkeep,
    steps::bank_and_normalize,
    steps::running_node,
    steps::tutorial,
    steps::pack_streamed,
    steps::market_passes,
    steps::waits,
    steps::graph_state,
];

/// Launch pass for one idle worker. Order: live snapshot (no decision
/// without it), invariants (`normalize`), the running node (market fill
/// wait, lost report), the tutorial gate, then the next ready node: the
/// planner decides (`launch_decide`), the drive performs and applies it.
pub(crate) fn maybe_launch(orch: &Arc<dyn Fleet>, job: &Arc<Job>) {
    let Some(mut pass) = steps::open(orch, job) else { return };
    for step in PASS_STEPS {
        if let Flow::Stop(save) = step(&mut pass) {
            pass.finish(save);
            return;
        }
    }
    let Some(mut picked) = pick_node(&mut pass) else { return };
    let mut fx = crate::brain_launch_fx::LiveFx::new(orch, job, &pass.live);
    let work = drive::Drive { life: &mut pass.life, inputs: &mut pass.inputs, ctx: &mut picked.ctx, position: picked.position, committed: picked.committed };
    let save = drive::drive(&mut fx, work);
    pass.finish(save);
}

/// Pick the next ready node, close it when its climb is reached or its
/// quest done, else build its resolve context. `None`: the pass ended
/// here (life saved as the step asked).
fn pick_node(pass: &mut LaunchPass) -> Option<Picked> {
    let position = match node::pick_next_ready(pass) {
        Ok(position) => position,
        Err(save) => {
            pass.finish(save);
            return None;
        }
    };
    if let Flow::Stop(save) = node::already_reached(pass, position) {
        pass.finish(save);
        return None;
    }
    let quests_done = match node::quest_done(pass, position) {
        Ok(quests_done) => quests_done,
        Err(save) => {
            pass.finish(save);
            return None;
        }
    };
    Some(node::build_ctx(pass, position, quests_done))
}


/// Folds a refused launch into the life on disk (`fold_refused_launch`)
/// and drops the launch grace mark.
pub(crate) fn refused_launch(account: &str, node_id: &str) {
    let Some(mut life) = load_life(account) else { return };
    if crate::brain_inflight::fold_refused_launch(&mut life, node_id) {
        push_history(&mut life, node_id, crate::brain_inflight::LAUNCH_REFUSED);
        save_or_log(&life);
        crate::logln!("brain {account}: {node_id} launch refused, back to ready (uncounted)");
    }
    launch_marks().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account);
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
    crate::brain_chain::note_launched(account);
    launch_marks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(account.to_string(), std::time::Instant::now());
}
