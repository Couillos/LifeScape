#![forbid(unsafe_code)]
//! Value of serving a demand, inputs included (2026-11-21). Live: crafters
//! farmed uncut gems by melee drops for 120k-232k ticks because a demand
//! milestone (`demand:diamond`) priced the cut gem against inputs at the
//! market reference, never against the ticks the bot spends obtaining
//! them. Each input costs the cheaper of two ways: buying it at a real
//! resting ask, or making it, valued at the bot's own ticks. The value of
//! serving is (bid - coins spent) / (ticks of the bot, inputs it makes
//! included), compared with what a tick of the bot's time is worth. No
//! price floor, no item list. Missing data (no way workable, a rate that
//! is 0 or unread, no price for an input that must be bought) decides
//! nothing.
//!
//! Pure: rates, levels and asks come in as arguments.

use crate::obtain_legs::production_legs;

/// Depth of the input recursion (cut gem -> uncut gem -> drop farm).
const MAX_DEPTH: usize = 4;

/// What serving one unit costs: coins spent and ticks of the bot's time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitPlan {
    pub coins: f64,
    pub ticks: f64,
}

/// Judgement of serving a demand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServeVerdict {
    /// Coins a tick of the bot's time earns serving, at least what a tick is worth.
    Pays { per_tick: f64 },
    /// Below what a tick of the bot's time is worth.
    Unpaying { per_tick: f64, time_value: f64 },
    /// No decision: nothing measured to judge on.
    NoData,
}

/// Inputs of the judgement.
pub struct ServeContext<'a> {
    /// Units per 6000 ticks of `(method, item)`; 0 or NaN: never launches.
    pub units: &'a dyn Fn(&str, &str) -> f64,
    /// Best resting ask of an object, `None` when no seller rests.
    pub ask_of: &'a dyn Fn(&str) -> Option<f64>,
    /// Coins a tick of the bot's time is worth.
    pub time_value: f64,
}

impl ServeContext<'_> {
    fn worth(&self, plan: UnitPlan) -> f64 {
        plan.coins + plan.ticks * self.time_value
    }

    /// Cheapest way to come by one unit of `item` (market ask or own making).
    fn input_plan(&self, item: &str, depth: usize) -> Option<UnitPlan> {
        let bought = (self.ask_of)(item).filter(|ask| ask.is_finite() && *ask > 0.0).map(|ask| UnitPlan { coins: ask, ticks: 0.0 });
        let made = if depth < MAX_DEPTH { self.make_plan(item, depth) } else { None };
        match (bought, made) {
            (Some(bought), Some(made)) => Some(if self.worth(bought) <= self.worth(made) { bought } else { made }),
            (bought, made) => bought.or(made),
        }
    }

    /// Cheapest own making of one unit of `item`: every workable leg, its
    /// own ticks plus its inputs (each the cheaper of buying or making).
    /// Levels the goal trains on the way are its own cost: no level gate.
    pub fn make_plan(&self, item: &str, depth: usize) -> Option<UnitPlan> {
        let mut best: Option<UnitPlan> = None;
        for leg in production_legs(item) {
            let per_6000 = (self.units)(&leg.method, item);
            if !per_6000.is_finite() || per_6000 <= 0.0 {
                continue;
            }
            let mut plan = UnitPlan { coins: 0.0, ticks: 6000.0 / per_6000 };
            let mut complete = true;
            for (input, count) in &leg.inputs {
                let Some(one) = self.input_plan(input, depth + 1) else {
                    complete = false;
                    break;
                };
                let count = (*count).max(1) as f64;
                plan.coins += one.coins * count;
                plan.ticks += one.ticks * count;
            }
            if complete && best.is_none_or(|known| self.worth(plan) < self.worth(known)) {
                best = Some(plan);
            }
        }
        best
    }

    /// Does selling `item` at `bid` coins a unit pay for the bot's time?
    pub fn verdict(&self, item: &str, bid: f64) -> ServeVerdict {
        let Some(plan) = self.make_plan(item, 0) else { return ServeVerdict::NoData };
        if !bid.is_finite() || !self.time_value.is_finite() || plan.ticks <= 0.0 {
            return ServeVerdict::NoData;
        }
        let per_tick = (bid - plan.coins) / plan.ticks;
        let time_value = self.time_value.max(crate::market_fit::MIN_COIN_RATE);
        if per_tick >= time_value {
            ServeVerdict::Pays { per_tick }
        } else {
            ServeVerdict::Unpaying { per_tick, time_value }
        }
    }
}

/// Open demand milestones (`demand:<item>` origin) serving which does not
/// pay the bot's time are set aside (A2 `set_aside`: blocked the maximum
/// number of times, kept in the ambition so they come back when prices
/// change). `verdict` judges one item; the milestone `running` (the open
/// goal's origin) is left for the caller to end when `wait_running` is
/// false. Returns `(milestone id, item, per tick, time value)` of the
/// milestones set aside.
pub fn set_aside_unpaying(ambition: &mut crate::ambition::Ambition, verdict: &dyn Fn(&str, &crate::ambition::Milestone) -> ServeVerdict, running: Option<&str>, may_end_running: bool) -> Vec<(String, String, f64, f64)> {
    use crate::ambition::{drawable, MilestoneStatus, Target, MILESTONE_MAX_BLOCKS};
    let mut aside = Vec::new();
    for stone in ambition.milestones.iter_mut().filter(|stone| drawable(stone)) {
        let Some(item) = stone.origin.strip_prefix("demand:").map(str::to_string) else { continue };
        if !matches!(stone.target, Target::Produce { .. }) || (running == Some(stone.id.as_str()) && !may_end_running) {
            continue;
        }
        if let ServeVerdict::Unpaying { per_tick, time_value } = verdict(&item, stone) {
            stone.status = MilestoneStatus::Blocked;
            stone.blocked_count = MILESTONE_MAX_BLOCKS;
            aside.push((stone.id.clone(), item, per_tick, time_value));
        }
    }
    aside
}

#[cfg(test)]
#[path = "serve_value_tests.rs"]
mod tests;
