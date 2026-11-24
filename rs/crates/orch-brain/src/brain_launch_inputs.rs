#![forbid(unsafe_code)]
//! Inputs of one launch pass (lot 1 of the launch refactor): the live
//! facts `maybe_launch` reads, filled into the planner's plain
//! `LaunchInputs` instead of being read piecemeal along the pass.
//!
//! Two reads. `gather` runs once, at the start of the pass, from the
//! live snapshot the pass already took: position, streamed flags, combat,
//! the stacks and the in-memory flags. `gather_market` reads what the
//! pass's own actions change (locks, listings, committed coins, the
//! fleet's sites, the pack as the job holds it now): it runs again at
//! every point marked `// lot 3:`, never once for the whole pass.

use std::collections::BTreeMap;

use lifescape_json::Value;
use lifescape_planner::{read_stacks, LaunchInputs};

use crate::brain_plan::{bank_types_of, live_combat, names_of, stacks_of};
use crate::jobs::Job;

/// The start-of-pass read from the pass's live snapshot (no lock, no
/// store, no market).
pub(crate) fn gather(job: &Job, live: &Value) -> LaunchInputs {
    LaunchInputs {
        position: crate::errors_model::position_of(live.get("position")),
        ctx_position: live.get("position").as_arr().and_then(|cells| {
            let part = |slot: usize| cells.get(slot).and_then(|cell| cell.as_i64());
            Some((part(1)?, part(2)?, part(0)?))
        }),
        inv_streamed: live.get("inv_streamed").as_bool() == Some(true),
        bank_streamed: live.get("bank_streamed").as_bool() == Some(true),
        combat: live_combat(live),
        bank_types: bank_types_of(live),
        stacks: stacks_of(live),
        worn: names_of(live, "worn"),
        inv: read_stacks(live, "inv"),
        bank: read_stacks(live, "bank"),
        wearing: read_stacks(live, "worn"),
        // The pass's plain data (lot 3), each as the pre-lot-3 pass read it
        // at its point of use; the production gather MUST set them (a
        // default `now: 0` stamped every history entry 0 and expired every
        // market fill wait on the spot, 2026-11-23).
        now: now_unix(),
        tick_ms: crate::market_eta::live_tick_ms(),
        levels: live_skill_levels(live),
        market_up: crate::market_service::shared().is_some(),
        filler_ticks: crate::brain_filler::FILLER_TICKS,
        // Memory-only marks of the account.
        recent_launch: crate::brain_launch::recent_launch(&job.account),
        report_in_flight: crate::brain_inflight::in_flight(&job.account),
        report_pending: report_pending_of(job),
        // The market state (`gather_market`, read at the `// lot 3:`
        // points, never at the start of the pass): empty until then. Every
        // field is named, so a new `LaunchInputs` field cannot be forgotten.
        locks: BTreeMap::new(),
        listed: BTreeMap::new(),
        commit_rows: Vec::new(),
        commit_flat: 0,
        crowd: Vec::new(),
        seats: Vec::new(),
    }
}

/// The report-pending flag as the idle detector reads it (a contended
/// job lock reads as pending).
fn report_pending_of(job: &Job) -> bool {
    job.inner.try_lock().map(|inner| inner.report_pending).unwrap_or(true)
}

/// Unix seconds now (the old `brain_launch::now_unix`).
pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Reads the clock again: the old pass read `now_unix()` at each point of
/// use, a pass that waits on the market must not decide on its start time.
pub(crate) fn refresh_clock(inputs: &mut LaunchInputs) {
    inputs.now = now_unix();
}

/// `brain_succession::live_level` of every skill the snapshot lists.
pub(crate) fn live_skill_levels(live: &Value) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for row in live.get("skills").as_arr().cloned().unwrap_or_default() {
        let Some(stat) = row.get("id").as_i64().and_then(|id| u8::try_from(id).ok()) else { continue };
        let name = lifescape_sdk::provenance::skill_name(stat).to_string();
        let level = crate::brain_succession::live_level(live, &name);
        out.push((name, level));
    }
    out
}

/// The launch grace mark of the account (memory only).
pub(crate) fn refresh_launch_mark(job: &Job, inputs: &mut LaunchInputs) {
    inputs.recent_launch = crate::brain_launch::recent_launch(&job.account);
}

/// The report flags the idle detector reads: queued in the inbox, kept
/// pending on a full inbox (a contended job lock reads as pending, as
/// the detector always did).
pub(crate) fn refresh_report_flags(job: &Job, inputs: &mut LaunchInputs) {
    inputs.report_in_flight = crate::brain_inflight::in_flight(&job.account);
    inputs.report_pending = report_pending_of(job);
}

/// What the pass's own actions change, read now: the job's pack as it
/// stands (a contended lock reads empty, as `holdings_count` did), the
/// locks and listings of the bot's resting sells, the coins its resting
/// buys commit, the fleet's sites.
pub(crate) fn gather_market(job: &Job, inputs: &mut LaunchInputs) {
    let live = job.inner.try_lock().map(|inner| inner.live_or_null()).unwrap_or_else(|_| crate::jobs::null_live());
    inputs.inv = read_stacks(&live, "inv");
    inputs.bank = read_stacks(&live, "bank");
    inputs.wearing = read_stacks(&live, "worn");
    inputs.locks = crate::brain_locks::locked_items(&job.account);
    inputs.listed.clear();
    inputs.commit_rows.clear();
    inputs.commit_flat = 0;
    if let Some(service) = crate::market_service::shared() {
        let (rows, deals) = {
            let store = service.lock_store();
            (store.account_orders(&job.account).unwrap_or_default(), store.open_deals_for_account(&job.account).unwrap_or_default())
        };
        for row in &rows {
            if row.side == lifescape_store::OrderSide::Sell && row.remaining > 0 && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) {
                *inputs.listed.entry(row.item_id).or_insert(0) += row.remaining.max(0);
            }
            if crate::brain_gear::cancellable(row) {
                inputs.commit_rows.push((row.item_id, crate::market_units::deal_coins(row.item_id, row.remaining, row.price.max(0))));
            }
        }
        // The coins a donor owes the makers (`fund-makers`) are spent too.
        inputs.commit_flat = crate::brain_gear::deal_coins_from(&job.account, &deals) + service.gifts_owed(&job.account);
    }
    // The market actions took time (a fill wait is read against `now`), the
    // service may have come up, the tick length is measured live.
    refresh_clock(inputs);
    inputs.tick_ms = crate::market_eta::live_tick_ms();
    inputs.market_up = crate::market_service::shared().is_some();
    inputs.crowd = crate::site_crowd::others(&job.account);
    inputs.seats = crate::site_crowd::seats(&job.account);
}
