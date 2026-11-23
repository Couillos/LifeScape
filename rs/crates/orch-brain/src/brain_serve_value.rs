#![forbid(unsafe_code)]
//! A demand is served only when it pays for the bot's time (2026-11-21).
//! Live: ~25 % of the fleet's work went to melee farms of uncut gems
//! (`side:demand:uncut_*` goals) at ~160 gems/h in all, while the gem
//! price crashed; an input-less object's margin is its whole bid, so
//! every bid "paid". The value comparison: per-unit margin (best bid
//! against the inputs at the market reference) times the units one tick
//! of the best way the bot can work yields, against the coins a tick of
//! the bot's time is worth (`brain_bids::bot_rate`,
//! `lifescape_planner::pays_its_way`). No price floor, no item list.
//!
//! - `paying_demand`: the demand items that pass, for the side goal draw;
//! - `serve_facts`: what the planner's `upkeep::unpaying_give` reads to give
//!   back an open `side:demand:` goal whose value fell below the bot's time
//!   (same close as the other releases of `brain_bid_release`). A `bid:` goal is not judged here: its
//!   answer was weighed against the asks it buys from and the training
//!   it carries (`bid_response::preempts`, rate >= the bot's coin rate);
//!   judging it on reference prices released 11 sound answers at the
//!   first deploy (strung amulets bought below the reference).

use lifescape_json::Value;
use lifescape_planner::ambition_catalog::DemandItem;
use lifescape_planner::upkeep::{ServeFacts, ServeQuery};
use lifescape_planner::Life;

/// Last error of the nodes an unpaying goal abandons.
pub const UNPAID_CAUSE: &str = lifescape_planner::upkeep::UNPAID_CAUSE;

/// History outcome (node `goal`) of a goal given back as unpaying:
/// `serve_unpaid:<origin>`.
pub const UNPAID_TAG: &str = lifescape_planner::upkeep::UNPAID_TAG;

/// Units one tick of the best way of `item` the bot can work yields
/// (`units`: units per 6000 ticks of a method for an object, 0 or NaN
/// when it never launches now). 0 when no leg is workable.
pub(crate) fn units_per_tick(item: &str, level_of: &dyn Fn(&str) -> u8, units: &dyn Fn(&str, &str) -> f64) -> f64 {
    lifescape_planner::obtain_legs::production_legs(item)
        .into_iter()
        .filter(|leg| leg.skill.as_ref().is_none_or(|(skill, level)| i64::from(level_of(skill).max(1)) >= *level))
        .map(|leg| units(&leg.method, item))
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .fold(0.0_f64, f64::max)
        / 6000.0
}

/// Names of the `demand` items serving which pays for this bot's time.
pub(crate) fn paying_demand(life: &Life, live: &Value, demand: &[DemandItem]) -> Vec<String> {
    crate::brain_bids::with_bot_state(life, live, |state| {
        let time_value = crate::brain_bids::bot_rate(life);
        demand
            .iter()
            .filter(|wanted| wanted.margin.is_some_and(|margin| lifescape_planner::pays_its_way(margin, units_per_tick(&wanted.item, state.level_of, state.units), time_value)))
            .map(|wanted| wanted.item.clone())
            .collect()
    })
}

/// What the decision to give back an open `side:demand:` goal reads: the
/// margin of its demand and, when there is one, the units a tick of the
/// bot yields serving it and a tick of its time (`upkeep::unpaying_give`
/// is the rule). Levels the goal trains on the way are its own cost (bid
/// answers carry their training): every leg counts. No margin read, no
/// facts: no data, no decision; the bot state is only read after a margin.
pub(crate) fn serve_facts(life: &Life, live: &Value, query: &ServeQuery) -> Option<ServeFacts> {
    let signals = crate::market_demand::signals_for(&life.account);
    let margin = crate::market_demand::serving_margin(&query.item, &signals)?;
    let (per_tick, time_value) = crate::brain_bids::with_bot_state(life, live, |state| (units_per_tick(&query.item, &|_| u8::MAX, state.units), crate::brain_bids::bot_rate(life)));
    Some(ServeFacts { margin, per_tick, time_value })
}

#[cfg(test)]
mod tests {
    use super::units_per_tick;

    /// The best workable leg sets the rate per tick (units per 6000
    /// ticks / 6000); a leg that never launches (0 or NaN) counts none.
    #[test]
    fn the_best_workable_leg_sets_the_rate() {
        let legs = lifescape_planner::obtain_legs::production_legs("uncut_sapphire");
        assert!(!legs.is_empty(), "uncut sapphire has production legs");
        let level = |_: &str| u8::MAX;
        let measured = |_: &str, _: &str| 12.0;
        assert!((units_per_tick("uncut_sapphire", &level, &measured) - 12.0 / 6000.0).abs() < 1e-12);
        let never = |_: &str, _: &str| f64::NAN;
        assert_eq!(units_per_tick("uncut_sapphire", &level, &never), 0.0);
    }
}
