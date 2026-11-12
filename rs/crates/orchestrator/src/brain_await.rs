#![forbid(unsafe_code)]
//! The market wait of an obtain node (split from `brain.rs`, rule 4):
//! a `market:buy` leg placed daemon-side keeps the node active, without
//! a worker, until its holdings say how it ended.

use lifescape_planner::{Life, NodeStatus};

use crate::brain::push_history;
use crate::brain_plan::holdings_count;
use crate::jobs::Job;

/// Market fills settle through other players: an unfilled order waits
/// at most this many ticks (wall time = ticks × tick length, D7).
pub(crate) const AWAIT_FILL_TICKS: u64 = 6000;

/// One pass over the obtain node at `position`, waiting `waited` seconds
/// for its buy of `count` `item`. Returns true while it keeps waiting
/// (nothing changed), false once the node moved on:
/// - holdings reach the count: done, the leftover bid closes;
/// - a partial buy landed (holdings above the launch baseline, the order
///   and its handover over): ready again, the next resolve produces or
///   buys the rest, uncounted;
/// - the deadline passed: the stale order goes, one failure counted.
pub(crate) fn settle_fill(job: &Job, life: &mut Life, position: usize, item: &str, count: i64, waited: i64) -> bool {
    let active_id = life.nodes[position].id.clone();
    let held = holdings_count(job, item);
    // Holdings at the placement (set at launch).
    let baseline = life.nodes[position].progress_current;
    let deadline = (AWAIT_FILL_TICKS * lifescape_sdk::waits::tick_ms() / 1000) as i64;
    if held >= count {
        life.nodes[position].progress_current = held;
        crate::brain_market::cancel_open_buys(&job.account, item);
        life.nodes[position].status = NodeStatus::Done;
        life.active = None;
        push_history(life, &active_id, "goal_reached");
    } else if held > baseline && crate::brain_market::buy_closed(&job.account, item) {
        crate::logln!("brain {}: {active_id} partial buy landed ({baseline} -> {held}/{count} {item}), re-resolving", job.account);
        life.nodes[position].progress_current = held;
        life.nodes[position].status = NodeStatus::Ready;
        life.nodes[position].method = None;
        life.active = None;
        push_history(life, &active_id, "partial_fill");
    } else if waited > deadline {
        // The stale resting order goes before the retry places a fresh
        // one (PRD3 lot 0: bots stacked two identical bids on the same axe).
        let closed = crate::brain_market::cancel_open_buys(&job.account, item);
        let text = format!("market order for {count} {item} unfilled after {AWAIT_FILL_TICKS} ticks (held {held}, {closed} cancelled)");
        let fold = lifescape_planner::record_failure(life, position, &text);
        push_history(life, &active_id, if fold == lifescape_planner::FailureFold::Abandoned { "abandoned" } else { "failed" });
    } else {
        return true;
    }
    false
}
