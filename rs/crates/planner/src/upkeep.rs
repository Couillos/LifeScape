#![forbid(unsafe_code)]
//! Life upkeep decisions (lot 4 of the launch refactor), the first
//! family: the goal give-backs. A bid answer whose sites refuse, an
//! answer off the bot's trade or beyond its climb, a side demand goal or
//! a demand milestone that no longer pays the bot's time, a goal an
//! earlier ambition drew: each rule DECIDES here, from the life and typed
//! facts (no I/O, no global, no clock but the `Clock` it is given), and
//! returns a `GiveBack` (or nothing). `apply_give_back` writes it into the
//! life, `finish_give_back` closes it once the orchestrator has drawn the
//! next goal (the draw reads the fleet and the market: it stays there).
//! The log lines come back as text, in the order they were printed.
//!
//! Rules keep the dated user decisions they carried in
//! `brain_bid_release`, `brain_serve_value` and `brain_milestone_value`.

use crate::ambition::{Ambition, Milestone};
use crate::launch_apply::push_history;
use crate::serve_value::ServeVerdict;
use crate::{abandon_goal_work, goal_closed, normalize, protected, GoalKind, Life};

/// Last error of the nodes a given-back bid answer abandons.
pub const RELEASE_CAUSE: &str = "bid answer given back: its production waited on full sites past one cooldown";
/// Last error of the nodes an answer off the bot's trade abandons.
pub const OFF_TRADE_CAUSE: &str = "bid answer off the bot's trade: its ambition follows the need (bid_trade)";
/// History outcome (node `goal`) of an answer given back as off the trade.
pub const OFF_TRADE_TAG: &str = "bid_off_trade:";
/// Last error of the nodes an answer beyond the climb of now abandons.
pub const BEYOND_REACH_CAUSE: &str = "bid answer drawn under the old rule: its level beyond the climb of now or its climb chosen by the old training score (bid_reach)";
/// History outcome (node `goal`) of an answer given back as beyond the climb.
pub const BEYOND_REACH_TAG: &str = "bid_beyond_reach:";
/// Last error of the nodes an unpaying goal abandons.
pub const UNPAID_CAUSE: &str = "demand goal given back: serving it no longer pays for the bot's time";
/// History outcome (node `goal`) of a goal given back as unpaying.
pub const UNPAID_TAG: &str = "serve_unpaid:";
/// Last error of the nodes of a goal given back as an unpaying milestone.
pub const UNPAID_MILESTONE_CAUSE: &str = "demand milestone set aside: serving it no longer pays for the bot's time";

/// The clock a rule reads: unix seconds and the daemon's tick length
/// (60 ms dev, 600 ms prod). Elapsed ticks are always measured from the
/// seconds, so a tick-rate change never jumps a throttle.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub now: i64,
    pub tick_ms: u64,
    /// Seconds of daemon downtime inside `[from, to]` (`orch-core::downtime`,
    /// `no_downtime` when none): a wait that penalises the bot (a bid given
    /// back after a long block) does not run while the daemon is stopped.
    pub down: fn(i64, i64) -> i64,
}

/// No downtime at all (tests, the plain clock).
pub fn no_downtime(_from: i64, _to: i64) -> i64 {
    0
}

impl Clock {
    /// Ticks elapsed since a history stamp (`brain_stuck::seconds_to_ticks`).
    pub fn ticks_since(&self, stamp: i64) -> u64 {
        ((self.now - stamp).max(0) as u64).saturating_mul(1000) / self.tick_ms.max(1)
    }

    /// Ticks waited since `stamp` without the downtime (a PENALISING
    /// timer: the stamp moves forward by the downtime inside the wait).
    pub fn awake_ticks_since(&self, stamp: i64) -> u64 {
        self.ticks_since(stamp + (self.down)(stamp, self.now))
    }
}

/// One goal given back: what to abandon, what to print, what to write,
/// and how the draw that follows is told.
#[derive(Debug, Clone, PartialEq)]
pub struct GiveBack {
    /// Last error of the nodes the goal's work abandons.
    pub cause: &'static str,
    /// Log line up to the abandoned nodes: `<head><n> nodes abandoned: <ids>`.
    pub head: String,
    /// History outcome (node `goal`) written after the log, when any.
    pub history: Option<String>,
    /// The goal's origin is dropped before the draw (a set-aside milestone).
    pub clear_origin: bool,
    /// Log line of a draw that found a goal (`{label}` is the goal).
    pub next: String,
    /// Log line of a draw that found nothing.
    pub nothing: String,
}

/// The nodes abandoned, the life normalized, the history entry written;
/// returns the log line. The orchestrator then draws the next goal
/// (`succeed_goal`) and calls `finish_give_back`.
pub fn apply_give_back(life: &mut Life, give: &GiveBack, now: i64) -> String {
    let abandoned = abandon_goal_work(life, give.cause);
    normalize(life);
    let line = format!("{}{} nodes abandoned: {}", give.head, abandoned.len(), abandoned.join(","));
    if let Some(outcome) = &give.history {
        push_history(life, "goal", outcome, now);
    }
    if give.clear_origin {
        life.goal_origin = None;
    }
    line
}

/// The life normalized after the draw; returns the line of its outcome.
pub fn finish_give_back(life: &mut Life, give: &GiveBack, drawn: Option<&str>) -> String {
    normalize(life);
    match drawn {
        Some(label) => give.next.replace("{label}", label),
        None => give.nothing.clone(),
    }
}

fn next_and_nothing(account: &str, origin: &str) -> (String, String) {
    (format!("brain {account}: next goal {{label}} (after giving back {origin})"), format!("brain {account}: nothing drawable after giving back {origin} (see the reason above)"))
}

/// A bid answer whose production every site refused past one cooldown
/// (2026-11-20). Waits while a node runs (its report decides first).
pub fn blocked_bid_give(life: &Life, clock: Clock) -> Option<GiveBack> {
    let bid = life.goal_origin.as_deref().is_some_and(|origin| origin.starts_with(crate::bid_response::BID_PREFIX));
    if !bid || life.active.is_some() {
        return None;
    }
    let ticks_since = |stamp: i64| clock.ticks_since(stamp);
    let cooled = crate::cooldown::cooled_keys(&life.history, &ticks_since);
    // The wait of the block does not run while the daemon is down; the
    // cooldowns only free the sites, they keep their plain clock.
    let awake = |stamp: i64| clock.awake_ticks_since(stamp);
    let blocked = crate::bid_release::blocked_bid(life, &cooled, &awake)?;
    let (next, nothing) = next_and_nothing(&life.account, &blocked.origin);
    Some(GiveBack {
        cause: RELEASE_CAUSE,
        head: format!(
            "brain {}: {} given back: {} waited {} ticks on refused sites (>= {}), no ban; ",
            life.account,
            blocked.origin,
            blocked.node,
            blocked.waited_ticks,
            crate::bid_release::BID_WAIT_RELEASE_TICKS
        ),
        history: Some(format!("{}{}|{}|{}", crate::bid_release::RELEASED_TAG, blocked.origin, blocked.node, blocked.waited_ticks)),
        clear_origin: false,
        next,
        nothing,
    })
}

/// A `bid:` goal off the bot's trade (2026-11-20, the answers drawn before
/// `bid_trade`): given back, the need-drawn ambition drawn again.
pub fn off_trade_give(life: &Life) -> Option<GiveBack> {
    if life.active.is_some() {
        return None;
    }
    let origin = crate::bid_trade::off_trade_answer(life)?;
    let kind = life.ambition.as_ref().map(|held| held.kind.clone()).unwrap_or_default();
    let (next, nothing) = next_and_nothing(&life.account, &origin);
    Some(GiveBack {
        cause: OFF_TRADE_CAUSE,
        head: format!("brain {}: {origin} given back: off the {kind}'s trade, no ban; ", life.account),
        history: Some(format!("{OFF_TRADE_TAG}{origin}")),
        clear_origin: false,
        next,
        nothing,
    })
}

/// A `bid:` goal drawn under the old rule whose object needs a level above
/// the bot's and its open level milestones (2026-11-20). `level_of`: the
/// bot's live levels.
pub fn beyond_reach_give(life: &Life, level_of: &dyn Fn(&str) -> u8) -> Option<GiveBack> {
    if life.active.is_some() {
        return None;
    }
    let found = crate::bid_reach::beyond_reach_answer(life, level_of)?;
    let (next, nothing) = next_and_nothing(&life.account, &found.origin);
    Some(GiveBack {
        cause: BEYOND_REACH_CAUSE,
        head: format!("brain {}: {} given back (drawn under the old rule): {}; no ban; ", life.account, found.origin, found.why),
        history: Some(format!("{BEYOND_REACH_TAG}{}|{}", found.origin, found.why)),
        clear_origin: false,
        next,
        nothing,
    })
}

/// A changed ambition replaces the goal an earlier one drew (D3,
/// 2026-11-19). Waits while a node runs; a side, bid or alchemy goal runs on.
pub fn stale_goal_give(life: &Life) -> Option<GiveBack> {
    if life.active.is_some() || !crate::ambition_review::goal_from_earlier_ambition(life) {
        return None;
    }
    let old = life.history.iter().rev().find(|entry| entry.node == "goal" && entry.outcome.starts_with(crate::ambition_review::NEXT_TAG)).map(|entry| entry.outcome.clone()).unwrap_or_default();
    let kind = life.ambition.as_ref().map(|ambition| ambition.kind.clone()).unwrap_or_default();
    Some(GiveBack {
        cause: crate::ambition_review::STALE_GOAL_CAUSE,
        head: format!("brain {}: goal ({old}) drawn by an earlier ambition, replaced for the {kind} ambition; ", life.account),
        history: None,
        clear_origin: false,
        next: format!("brain {}: next goal {{label}} ({kind})", life.account),
        nothing: format!("brain {}: nothing drawable for the {kind} ambition yet (see the reason above)", life.account),
    })
}

/// The open side demand goal a rule may give back: `(origin, item)`.
fn served_item(life: &Life) -> Option<(String, String)> {
    let origin = life.goal_origin.as_deref()?;
    let item = origin.strip_prefix(crate::SIDE_PREFIX).and_then(|rest| rest.strip_prefix("demand:"))?;
    match &life.goal {
        GoalKind::Produce { item: goal_item, .. } if goal_item == item => Some((origin.to_string(), item.to_string())),
        _ => None,
    }
}

/// The open `side:demand:` goal whose value the orchestrator must read
/// (2026-11-21: gem farms while the price crashed). `None`: nothing to judge
/// (a node runs, an ironman, a closed goal, another origin).
#[derive(Debug, Clone, PartialEq)]
pub struct ServeQuery {
    pub origin: String,
    pub item: String,
}

pub fn serve_query(life: &Life) -> Option<ServeQuery> {
    if life.active.is_some() || life.profile.ironman || goal_closed(&life.history) {
        return None;
    }
    let (origin, item) = served_item(life)?;
    Some(ServeQuery { origin, item })
}

/// What the orchestrator read for the query: the unit margin of the
/// demand, the units one tick of the bot yields serving it, and a tick of
/// the bot's time in coins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServeFacts {
    pub margin: f64,
    pub per_tick: f64,
    pub time_value: f64,
}

/// Gives back the side demand goal when serving it no longer pays. No data
/// (`None`: no margin read) or no rate (`per_tick <= 0`), no decision.
pub fn unpaying_give(life: &Life, query: &ServeQuery, facts: Option<ServeFacts>) -> Option<GiveBack> {
    let ServeFacts { margin, per_tick, time_value } = facts?;
    if per_tick <= 0.0 || crate::pays_its_way(margin, per_tick, time_value) {
        return None;
    }
    let origin = &query.origin;
    let (next, nothing) = next_and_nothing(&life.account, origin);
    Some(GiveBack {
        cause: UNPAID_CAUSE,
        head: format!("brain {}: {origin} given back: {margin:.0} coins a unit x {per_tick:.5} units a tick < a tick worth {time_value:.2}, no ban; ", life.account),
        history: Some(format!("{UNPAID_TAG}{origin}")),
        clear_origin: false,
        next,
        nothing,
    })
}

fn exempt(life: &Life) -> bool {
    life.profile.ironman || protected(&life.account)
}

fn has_open_demand(life: &Life) -> bool {
    life.ambition.as_ref().is_some_and(|ambition| ambition.milestones.iter().any(|stone| crate::drawable(stone) && stone.origin.starts_with("demand:")))
}

/// The open demand milestones the orchestrator must judge, `(id, item)`
/// in ladder order (2026-11-21: gem farms of crafters). `None`: protected
/// and ironmen are left alone, no open demand milestone.
pub fn milestone_query(life: &Life) -> Option<Vec<(String, String)>> {
    if exempt(life) || !has_open_demand(life) {
        return None;
    }
    let ambition = life.ambition.as_ref()?;
    Some(
        ambition
            .milestones
            .iter()
            .filter(|stone: &&Milestone| crate::drawable(stone))
            .filter_map(|stone| stone.origin.strip_prefix("demand:").map(|item| (stone.id.clone(), item.to_string())))
            .collect(),
    )
}

/// Milestones set aside: the cooling entries written and their lines, and
/// the give-back of the running goal when it served one of them.
#[derive(Debug, Clone, PartialEq)]
pub struct MilestoneAside {
    pub lines: Vec<String>,
    pub give: Option<GiveBack>,
}

/// Sets aside the milestones judged unpaying (A2: kept, resumed when
/// nothing else is drawable; cooled like any set-aside one, 2026-11-23);
/// a goal of one waits for its node to end, then is given back.
pub fn milestone_aside(life: &mut Life, verdicts: &[(String, ServeVerdict)], now: i64) -> Option<MilestoneAside> {
    let account = life.account.clone();
    let running = life.goal_origin.clone().filter(|_| !goal_closed(&life.history));
    let may_end_running = life.active.is_none();
    let ambition: &mut Ambition = life.ambition.as_mut()?;
    let verdict = |_: &str, stone: &Milestone| verdicts.iter().find(|(id, _)| *id == stone.id).map(|(_, verdict)| *verdict).unwrap_or(ServeVerdict::NoData);
    let aside = crate::serve_value::set_aside_unpaying(ambition, &verdict, running.as_deref(), may_end_running);
    let kind = ambition.kind.clone();
    if aside.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for (id, item, per_tick, time_value) in &aside {
        let key = crate::milestone_cool_key(&kind, id);
        push_history(life, crate::cooldown::COOL_NODE, &crate::cooldown::cool_entry(&key, crate::AMBITION_CHANGE_TICKS, "set aside, unpaying"), now);
        lines.push(format!("brain {account}: milestone {id} (demand:{item}) set aside, ambition {kind} kept: {per_tick:.4} coins a tick of the bot < a tick worth {time_value:.2}, inputs bought at a real ask or made at its own ticks, no ban"));
    }
    let running_aside = running.as_ref().is_some_and(|origin| aside.iter().any(|entry| &entry.0 == origin));
    let give = running_aside.then(|| {
        let origin = running.unwrap_or_default();
        GiveBack {
            cause: UNPAID_MILESTONE_CAUSE,
            head: format!("brain {account}: goal of {origin} given back, "),
            history: Some(format!("{UNPAID_TAG}{origin}")),
            clear_origin: true,
            next: format!("brain {account}: next goal {{label}} (after setting {origin} aside)"),
            nothing: format!("brain {account}: nothing drawable after setting {origin} aside (see the reason above)"),
        }
    });
    Some(MilestoneAside { lines, give })
}

/// Open work of earlier goals nothing of the current goal waits on leaves,
/// persisted lives included (2026-11-22: 34 stale nodes). Returns the line
/// to print and writes the abandons; `None` when nothing was stale.
pub fn stale_work_line(life: &mut Life) -> Option<String> {
    let stale = crate::abandon_stale_goal_work(life, "work of an earlier goal, no current node waits on it");
    if stale.is_empty() {
        return None;
    }
    Some(format!("brain {}: {} open nodes of earlier goals abandoned: {}", life.account, stale.len(), stale.join(",")))
}

#[cfg(test)]
#[path = "upkeep_tests.rs"]
mod tests;
