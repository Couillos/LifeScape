#![forbid(unsafe_code)]
//! A bid answer whose production every site refuses is given back
//! (2026-11-20; rule and delay: `lifescape_planner::bid_release`). Live:
//! lily_north's `bid:mithril_ore` waited on `full:farm:mithril_ore` for
//! the whole hour, fillers meanwhile. Here: the history is read with the
//! daemon's tick length, the goal's work is abandoned (the running node,
//! posted market buys, gear steps and the janitor stay:
//! `abandon_goal_work`), the goal closes without any ban (`succeed_goal`
//! with `stuck = false`, like a catalogue-dropped goal), and the draw that
//! follows frees the lot's claims, purchases and seats
//! (`brain_succession::draw_next`) and returns to the ambition.

#[cfg(test)]
use lifescape_json::Value;
use lifescape_planner::Life;

/// Last error of the nodes a given-back answer abandons.
pub const RELEASE_CAUSE: &str = lifescape_planner::upkeep::RELEASE_CAUSE;
/// Last error of the nodes an answer off the bot's trade abandons.
pub const OFF_TRADE_CAUSE: &str = lifescape_planner::upkeep::OFF_TRADE_CAUSE;
/// History outcome (node `goal`) of an answer given back as off the
/// trade: `bid_off_trade:<origin>`.
pub const OFF_TRADE_TAG: &str = lifescape_planner::upkeep::OFF_TRADE_TAG;
/// Last error of the nodes an answer beyond the climb of now abandons.
pub const BEYOND_REACH_CAUSE: &str = lifescape_planner::upkeep::BEYOND_REACH_CAUSE;
/// History outcome (node `goal`) of an answer given back as beyond the
/// climb of now: `bid_beyond_reach:<origin>|<why>`.
pub const BEYOND_REACH_TAG: &str = lifescape_planner::upkeep::BEYOND_REACH_TAG;

// The three releases are decided by the planner (`upkeep::blocked_bid_give`,
// `off_trade_give`, `beyond_reach_give`) and driven by
// `brain_upkeep_drive`; these entry points run them over the daemon's world
// without a worker (the unit tests).

/// Gives back the current bid answer when its production has waited past
/// the delay. True when the life changed.
#[cfg(test)]
pub(crate) fn release_blocked_bid(life: &mut Life, live: &Value) -> bool {
    crate::brain_upkeep_drive::release_blocked_bid(&crate::brain_upkeep_fx::LiveUpkeepFx::bare(live), life)
}

/// Gives back a `bid:` goal whose object is off the bot's trade.
#[cfg(test)]
pub(crate) fn release_off_trade(life: &mut Life, live: &Value) -> bool {
    crate::brain_upkeep_drive::release_off_trade(&crate::brain_upkeep_fx::LiveUpkeepFx::bare(live), life)
}

/// Everything `account`'s bid answer posted ends: the lot's claim and
/// its runs' claims, its purchases, its engaged seats and its launch
/// seat (review 2026-11-20: a role switch to market maker, an operator
/// `close` or `unload`). A later pass re-engages a bid goal still open
/// (`reengage_after_restart`).
pub(crate) fn free_board(account: &str) {
    crate::market_claims::release(account);
    crate::site_crowd::disengage(account);
    crate::site_crowd::note(account, None, "");
    restored().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(account);
}

/// Accounts whose bid goal's engagement was checked since the daemon
/// started (or since their board was freed).
fn restored() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static CHECKED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    CHECKED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// A daemon restart forgets the engaged seats (memory only): the first
/// pass that meets an open `bid:` goal without any re-engages the seats
/// its lot takes now (`brain_bids::lot_seats`), once per account, so the
/// first hour after a deploy does not send extra bots onto taken fields.
/// True when seats were engaged.
pub(crate) fn reengage_after_restart(life: &Life, live: &lifescape_json::Value) -> bool {
    reengage_with(life, &|item, count| crate::brain_bids::lot_seats(life, live, item, count))
}

/// `reengage_after_restart` with the lot's seats read by `seats_of`
/// (item, count).
fn reengage_with(life: &Life, seats_of: &dyn Fn(&str, i64) -> Vec<(String, (i64, i64, i64))>) -> bool {
    if !lifescape_planner::upkeep_ambition::reengage_candidate(life) {
        return false;
    }
    if !restored().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(life.account.clone()) || crate::site_crowd::is_engaged(&life.account) {
        return false;
    }
    let Some((item, count)) = lifescape_planner::upkeep_ambition::reengage_lot(life) else { return false };
    let seats = seats_of(&item, count);
    if seats.is_empty() {
        return false;
    }
    crate::site_crowd::engage(&life.account, &seats, crate::market_claims::MAX_CLAIM_SECS);
    let held: Vec<String> = seats.iter().map(|(method, site)| format!("{method}@{},{},{}", site.0, site.1, site.2)).collect();
    crate::logln!("brain {}: {} re-engages its seats after a restart: [{}]", life.account, life.goal_origin.as_deref().unwrap_or("?"), held.join(", "));
    true
}

#[cfg(test)]
#[path = "brain_bid_release_tests.rs"]
mod tests;
