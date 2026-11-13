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
//! - with none, runs a FILLER: one short money window (`FILLER_TICKS`),
//!   coin-dropping spots first (the wait is often a wallet short of a few
//!   coins), steered off banned skills. Its node (`fill-<id>`) is not in
//!   the graph: its report is ignored by the fold, nothing is counted.

use std::sync::Arc;

use lifescape_json::Value;
use lifescape_planner::{Life, Node, NodeKind, NodeStatus};

use crate::brain::push_history;
use crate::daemon::Orchestrator;
use crate::jobs::Job;

/// Ticks of one filler window (then the pass rechecks the waits).
pub const FILLER_TICKS: u64 = 1500;

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
pub(crate) fn settle_parked(job: &Job, life: &mut Life) -> (bool, Vec<String>) {
    let now = now_secs();
    let mut changed = false;
    let mut waiting = Vec::new();
    for (_, node_id, since) in parked_waits(life) {
        let Some(position) = life.nodes.iter().position(|node| node.id == node_id) else { continue };
        let NodeKind::ObtainItems { item, count } = life.nodes[position].kind.clone() else { continue };
        if crate::brain_await::settle_fill(job, life, position, &item, count, now - since) {
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

/// One filler window while `waiting` rests on the book and the graph has
/// nothing else ready. Returns true when a run was queued (the caller
/// saved the life first). Never through a ban; nothing workable is
/// logged (typed reason), never a silent idle.
pub(crate) fn launch_filler(orch: &Arc<Orchestrator>, job: &Arc<Job>, life: &mut Life, live: &Value, waiting: &[String]) -> bool {
    let Some(first) = waiting.first() else { return false };
    // A filler just queued: the worker may still read idle (one run at a time).
    if crate::brain_launch::recent_launch(&job.account) {
        return false;
    }
    let coins = crate::brain_plan::holdings_count(job, "coins");
    let mut node = Node::new(&format!("fill-{first}"), NodeKind::HaveCoins { amount: coins.saturating_add(1) }, Vec::new(), 0);
    let mut ctx = crate::brain_plan::ready_ctx(life, &node);
    ctx.coins = coins;
    ctx.tools = crate::brain_plan::carried_tools(job);
    ctx.bank_known = live.get("bank_streamed").as_bool() == Some(true);
    ctx.combat = crate::brain_plan::live_combat(live);
    ctx.position = live.get("position").as_arr().and_then(|cells| {
        let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
        Some((part(1)?, part(2)?, part(0)?))
    });
    crate::brain_market::fill_ctx(&mut ctx, &node, live);
    lifescape_planner::resolve_ready(&mut node, &ctx);
    let banned = lifescape_planner::banned_skills(&life.history, &&|stamp: i64| crate::brain_stuck::seconds_to_ticks(now_secs() - stamp));
    if let Err(reason) = lifescape_planner::steer_off_bans(&mut node, &ctx, &banned) {
        note_idle(life, &format!("{} awaiting its fill, no filler: {reason}", waiting.join(",")));
        return false;
    }
    let method = match (&node.status, node.method.clone()) {
        (NodeStatus::Blocked { reason }, _) => {
            note_idle(life, &format!("{} awaiting its fill, filler blocked: {reason}", waiting.join(",")));
            return false;
        }
        (_, Some(method)) if !method.is_empty() => method,
        _ => {
            note_idle(life, &format!("{} awaiting its fill, no money method workable", waiting.join(",")));
            return false;
        }
    };
    let Some((script, args)) = lifescape_planner::map_method(&method, &node, Some((0, FILLER_TICKS))) else {
        note_idle(life, &format!("{} awaiting its fill, filler {method} maps to no script", waiting.join(",")));
        return false;
    };
    if !crate::registry::script_known(&script) {
        note_idle(life, &format!("{} awaiting its fill, filler script {script} unknown", waiting.join(",")));
        return false;
    }
    crate::logln!("brain {}: {} awaiting its fill, working {method} meanwhile ({FILLER_TICKS} ticks): {script} {}", job.account, waiting.join(","), args.join(" "));
    push_history(life, &node.id, "filler");
    if let Err(error) = crate::brain::save_life(life) {
        crate::logln!("{error}");
    }
    crate::brain_launch::note_launch(&job.account);
    let values: Vec<Value> = args.iter().map(|arg| lifescape_json::str_value(arg)).collect();
    let reply = orch.run_job(&job.account, &script, &values);
    if reply.get("ok").as_bool() != Some(true) {
        crate::logln!("brain {}: filler launch refused: {}", job.account, reply.get("error").as_str().unwrap_or("?"));
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
