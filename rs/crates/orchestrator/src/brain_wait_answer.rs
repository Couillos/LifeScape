#![forbid(unsafe_code)]
//! The hybrid (user 2026-11-20, « oui implémente ça »): an ambition goal
//! that waits on a buy sets itself aside for a market answer of ANY
//! trade (rule and live numbers: `lifescape_planner::bid_trade` module
//! doc). Trigger: the goal comes from one of the ambition's milestones,
//! no node runs, nothing but parked buys is ready (the filler's moment,
//! `brain_launch`), and the oldest parked buy has waited one filler
//! window (`brain_filler::FILLER_TICKS`) at least: a buy that fills at
//! once never sets the goal aside. A whole-market lot must pay now
//! (`brain_bids::whole_market_lot`), else the filler runs as before.
//!
//! Set aside: the goal's work is abandoned except the buys really posted
//! (they stay on the book, their fill lands in the bank,
//! `abandon_goal_work`), the milestone link is dropped BEFORE the close
//! (`on_goal_closed` reads `Unlinked`: the milestone is neither done nor
//! blocked), the close is a plain `done:`, then the draw weighs the whole
//! market (`draw_next_with(.., true)`); an answer drawn so is marked
//! `wait_answer:<origin>` (never given back as off the trade). The draw
//! after the answer returns to the ambition (the buy left on the book is
//! no work of the answer: `lifecycle::earlier_wait`, the answer's graph
//! finishes without it): a level milestone resumes from the live levels;
//! a production milestone is credited with what its goal made
//! (`bid_trade::credit_made`) and asks the rest only.

use lifescape_json::Value;
use lifescape_planner::Life;

/// Last error of the nodes a set-aside goal abandons.
pub const WAIT_CAUSE: &str = "ambition goal set aside: it waits on a buy, a market answer meanwhile";

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Sets the waiting ambition goal aside for a whole-market answer when
/// the module's trigger holds. True when the life changed (save it).
pub(crate) fn answer_while_waiting(life: &mut Life, live: &Value) -> bool {
    let now = now_secs();
    let ticks_since = |stamp: i64| crate::brain_stuck::seconds_to_ticks(now - stamp);
    set_aside_with(life, live, &ticks_since, &|life| crate::brain_bids::whole_market_lot(life, live), &mut |life| crate::brain_succession::draw_next_with(life, live, true))
}

/// True when the trigger holds (module doc), the lot aside.
fn waits_long(life: &Life, ticks_since: &dyn Fn(i64) -> u64) -> bool {
    if life.active.is_some() || lifescape_planner::quoting(life) || lifescape_planner::is_alcher(life) {
        return false;
    }
    let Some(origin) = life.goal_origin.as_deref() else { return false };
    let milestone = life.ambition.as_ref().is_some_and(|held| held.milestones.iter().any(|stone| stone.id == origin));
    if !milestone || lifescape_planner::goal_closed(&life.history) {
        return false;
    }
    // The current goal's own buys only: one left on the book by an
    // earlier goal (`earlier_wait`) never sets the next one aside.
    let floor = life.memory.floor.unwrap_or(0);
    crate::brain_filler::parked_waits(life)
        .iter()
        .filter(|(_, id, _)| lifescape_planner::lifecycle_retire::node_number(id).is_some_and(|number| number >= floor))
        .map(|(_, _, since)| ticks_since(*since))
        .max()
        .is_some_and(|waited| waited >= crate::brain_filler::FILLER_TICKS)
}

/// `answer_while_waiting` with the lot check and the draw given (tests).
fn set_aside_with(life: &mut Life, live: &Value, ticks_since: &dyn Fn(i64) -> u64, lot_ready: &dyn Fn(&Life) -> bool, draw: &mut dyn FnMut(&mut Life) -> Option<String>) -> bool {
    if !waits_long(life, ticks_since) || !lot_ready(life) {
        return false;
    }
    let milestone = life.goal_origin.clone().unwrap_or_default();
    // A production milestone keeps what its goal made (the redraw asks
    // the rest only, `goal_for_milestone`).
    let credited = lifescape_planner::bid_trade::credit_made(life, &milestone);
    // Unlinked BEFORE the close: the milestone is neither done nor blocked.
    life.goal_origin = None;
    let abandoned = lifescape_planner::abandon_goal_work(life, WAIT_CAUSE);
    lifescape_planner::normalize(life);
    crate::logln!(
        "brain {}: milestone {milestone} waits on a buy, set aside for a market answer ({credited} units credited); {} nodes abandoned: {}",
        life.account,
        abandoned.len(),
        abandoned.join(",")
    );
    crate::brain_succession::close_goal(life, live, false);
    let drawn = draw(life);
    let origin = life.goal_origin.clone().unwrap_or_default();
    if drawn.is_some() && origin.starts_with(lifescape_planner::bid_response::BID_PREFIX) {
        crate::brain::push_history(life, "goal", &format!("{}{origin}", lifescape_planner::bid_trade::WAIT_ANSWER_TAG));
    }
    match drawn {
        Some(label) => crate::logln!("brain {}: next goal {label} while milestone {milestone} waits", life.account),
        None => crate::logln!("brain {}: nothing drawable while milestone {milestone} waits (see the reason above)", life.account),
    }
    lifescape_planner::normalize(life);
    true
}

#[cfg(test)]
#[path = "brain_wait_answer_tests.rs"]
mod tests;
