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

use lifescape_json::Value;
use lifescape_planner::Life;

/// Last error of the nodes a given-back answer abandons.
pub const RELEASE_CAUSE: &str = "bid answer given back: its production waited on full sites past one cooldown";

/// Gives back the current bid answer when its production has waited
/// past the delay. Waits while a node runs (its report decides first).
/// True when the life changed (save it).
pub(crate) fn release_blocked_bid(life: &mut Life, live: &Value) -> bool {
    let bid = life.goal_origin.as_deref().is_some_and(|origin| origin.starts_with(lifescape_planner::bid_response::BID_PREFIX));
    if !bid || life.active.is_some() {
        return false;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    let cooled = crate::brain_cool::active(life);
    let Some(blocked) = lifescape_planner::bid_release::blocked_bid(life, &cooled, &ticks_since) else { return false };
    let abandoned = lifescape_planner::abandon_goal_work(life, RELEASE_CAUSE);
    lifescape_planner::normalize(life);
    crate::logln!(
        "brain {}: {} given back: {} waited {} ticks on refused sites (>= {}), no ban; {} nodes abandoned: {}",
        life.account,
        blocked.origin,
        blocked.node,
        blocked.waited_ticks,
        lifescape_planner::bid_release::BID_WAIT_RELEASE_TICKS,
        abandoned.len(),
        abandoned.join(",")
    );
    crate::brain::push_history(life, "goal", &format!("{}{}|{}|{}", lifescape_planner::bid_release::RELEASED_TAG, blocked.origin, blocked.node, blocked.waited_ticks));
    // Done, never stuck: no ban, no milestone touched (a bid goal has
    // none); the draw frees the claims, purchases and seats.
    match crate::brain_succession::succeed_goal(life, live, false) {
        Some(label) => crate::logln!("brain {}: next goal {label} (after giving back {})", life.account, blocked.origin),
        None => crate::logln!("brain {}: nothing drawable after giving back {} (see the reason above)", life.account, blocked.origin),
    }
    lifescape_planner::normalize(life);
    true
}

/// Last error of the nodes an answer off the bot's trade abandons.
pub const OFF_TRADE_CAUSE: &str = "bid answer off the bot's trade: its ambition follows the need (bid_trade)";

/// History outcome (node `goal`) of an answer given back as off the
/// trade: `bid_off_trade:<origin>`.
pub const OFF_TRADE_TAG: &str = "bid_off_trade:";

/// Gives back a `bid:` goal whose object is off the bot's trade
/// (`lifescape_planner::bid_trade`, 2026-11-20): the answers drawn before
/// the rule (60 of 63 bots live). Waits while a node runs. Same close as
/// a blocked answer: work abandoned, no ban, the draw frees the claims
/// and returns to the ambition; no shun (the draw no longer offers the
/// object). True when the life changed (save it).
pub(crate) fn release_off_trade(life: &mut Life, live: &Value) -> bool {
    if life.active.is_some() {
        return false;
    }
    let Some(origin) = lifescape_planner::bid_trade::off_trade_answer(life) else { return false };
    let kind = life.ambition.as_ref().map(|held| held.kind.clone()).unwrap_or_default();
    let abandoned = lifescape_planner::abandon_goal_work(life, OFF_TRADE_CAUSE);
    lifescape_planner::normalize(life);
    crate::logln!("brain {}: {origin} given back: off the {kind}'s trade, no ban; {} nodes abandoned: {}", life.account, abandoned.len(), abandoned.join(","));
    crate::brain::push_history(life, "goal", &format!("{OFF_TRADE_TAG}{origin}"));
    match crate::brain_succession::succeed_goal(life, live, false) {
        Some(label) => crate::logln!("brain {}: next goal {label} (after giving back {origin})", life.account),
        None => crate::logln!("brain {}: nothing drawable after giving back {origin} (see the reason above)", life.account),
    }
    lifescape_planner::normalize(life);
    true
}

/// Last error of the nodes an answer beyond the climb of now abandons.
pub const BEYOND_REACH_CAUSE: &str = "bid answer drawn under the old rule: its level beyond the climb of now or its climb chosen by the old training score (bid_reach)";

/// History outcome (node `goal`) of an answer given back as beyond the
/// climb of now: `bid_beyond_reach:<origin>|<why>`.
pub const BEYOND_REACH_TAG: &str = "bid_beyond_reach:";

/// Gives back a `bid:` goal drawn under the old rule whose object needs a
/// level above the bot's and above its open level milestones, or whose
/// graph still climbs a level (`lifescape_planner::bid_reach`,
/// 2026-11-20): the answers drawn while the ladder's top counted the
/// training free and the training score credited earnings. Waits while a
/// node runs. Same close as an answer off the
/// trade: work abandoned (its posted buys left to the need release), no
/// ban, no shun (the draw weighs the object again under the new rule).
/// True when the life changed (save it).
pub(crate) fn release_beyond_reach(life: &mut Life, live: &Value) -> bool {
    if life.active.is_some() {
        return false;
    }
    let level_of = |skill: &str| crate::brain_succession::live_level(live, skill);
    let Some(found) = lifescape_planner::bid_reach::beyond_reach_answer(life, &level_of) else { return false };
    let abandoned = lifescape_planner::abandon_goal_work(life, BEYOND_REACH_CAUSE);
    lifescape_planner::normalize(life);
    crate::logln!(
        "brain {}: {} given back (drawn under the old rule): {}; no ban; {} nodes abandoned: {}",
        life.account,
        found.origin,
        found.why,
        abandoned.len(),
        abandoned.join(",")
    );
    crate::brain::push_history(life, "goal", &format!("{BEYOND_REACH_TAG}{}|{}", found.origin, found.why));
    match crate::brain_succession::succeed_goal(life, live, false) {
        Some(label) => crate::logln!("brain {}: next goal {label} (after giving back {})", life.account, found.origin),
        None => crate::logln!("brain {}: nothing drawable after giving back {} (see the reason above)", life.account, found.origin),
    }
    lifescape_planner::normalize(life);
    true
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
pub(crate) fn reengage_after_restart(life: &Life, live: &Value) -> bool {
    reengage_with(life, &|item, count| crate::brain_bids::lot_seats(life, live, item, count))
}

/// `reengage_after_restart` with the lot's seats read by `seats_of`
/// (item, count).
fn reengage_with(life: &Life, seats_of: &dyn Fn(&str, i64) -> Vec<(String, (i64, i64, i64))>) -> bool {
    if !life.goal_origin.as_deref().is_some_and(|origin| origin.starts_with(lifescape_planner::bid_response::BID_PREFIX)) || lifescape_planner::goal_closed(&life.history) {
        return false;
    }
    if !restored().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(life.account.clone()) || crate::site_crowd::is_engaged(&life.account) {
        return false;
    }
    let lifescape_planner::GoalKind::Produce { item, count } = &life.goal else { return false };
    let seats = seats_of(item, *count);
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
