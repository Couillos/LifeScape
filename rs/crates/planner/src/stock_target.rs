//! Future stock (plan marché M6, decision Q5 2026-11-16): a bot bids
//! ahead, patiently, for what its planned work will burn — the current
//! work slice and the next one, never the whole ambition.
//!
//! - work slices (`work_slices`): the open training tranches (reach a
//!   level) and money nodes of the graph, the active one first, then the
//!   graph's order; `STOCK_SLICES` of them. A tranche starts at the live
//!   level (the current one) or where the open tranche before it in the
//!   same skill ends; its method is the node's, else its first
//!   alternative, else the tranche method of its start level;
//! - duration (`slice_ticks`): a tranche lasts the XP it must earn at the
//!   method's XP per 6000 ticks (derived from the published yield); a
//!   money node its drawn engagement (the mean of the draw before it is
//!   drawn), a `have_coins` node its launch window;
//! - consumption (`consumed_per_6000`): the signed OUTFLOWS of the
//!   consumable causes of the yield (plan rendements §8: input, ammo,
//!   rune, bait, supply, food) per 6000 ticks; food eaten counts as
//!   hitpoints, re-expressed in the food the bot buys (the best fitted
//!   food it affords, `melee_ladder::affordable_food`, as the food gate);
//! - target (`slice_needs`): Σ consumption × slice ticks / 6000;
//! - market affinity (`bought_share`, D13): an autarkic bot (under
//!   `AUTARKY_BELOW`) bids nothing and farms (its tranche's obtain nodes
//!   do); above, the share it buys ahead grows to the whole target at 1;
//! - the plan (`stock_plan`): per object, held free (plus in flight) at
//!   the target = met (a resting bid goes, `need_met`); a resting stock
//!   bid is kept (its concession walk goes on, its ceiling follows); over
//!   the refill point (`REFILL_AT` of the target) the bot waits; under
//!   it, one bid for the rest of the target, capped by the stock budget
//!   (`STOCK_BUDGET_PCT` of the free wallet), the current slice's objects
//!   first; an object without a public price is never bid;
//! - withdrawal (`withdrawal`): an object no line wants any more (the
//!   plan changed, the need disappeared) loses its stock bid
//!   (`need_gone`).
//!
//! Pure: the brain reads the yields, the book and the wallet
//! (`orchestrator::brain_stock`).

use crate::model::{Life, NodeKind, NodeStatus};

/// Work slices stocked ahead (Q5: the current one and the next).
pub const STOCK_SLICES: usize = 2;
/// Market affinity under which a bot buys nothing ahead (D13: an
/// autarkic bot farms its consumables).
pub const AUTARKY_BELOW: f64 = 0.25;
/// Refill point: share of the target under which a new stock bid goes
/// up (an (s, S) policy: wait over it, order up to the target under it).
pub const REFILL_AT: f64 = 0.5;
/// Share of the free wallet the stock bids may commit.
pub const STOCK_BUDGET_PCT: i64 = 50;
/// Work ticks a `have_coins` launch works (`brain_launch`: one window).
pub const HAVE_COINS_WINDOW_TICKS: f64 = 6000.0;
/// Yield causes that consume a stock (plan rendements §8).
pub const CONSUMED_CAUSES: [&str; 6] = ["input", "ammo", "rune", "bait", "supply", "food"];
/// Cancel reason of a stock bid whose object left the plan.
pub const NEED_GONE: &str = "need_gone";
/// Cancel reason of a stock bid whose target the holdings reach.
pub const NEED_MET: &str = "need_met";

/// One piece of planned work.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkSlice {
    pub node: String,
    pub method: String,
    /// Skill of a tranche (`None`: a money node).
    pub skill: Option<String>,
    pub from: u8,
    pub to: u8,
    /// Fixed duration (money nodes); `None`: the tranche's XP decides.
    pub window_ticks: Option<f64>,
}

fn open(status: &NodeStatus) -> bool {
    !matches!(status, NodeStatus::Done | NodeStatus::Abandoned)
}

fn node_method(node: &crate::model::Node) -> Option<String> {
    node.method.clone().or_else(|| node.alternatives.first().map(|alt| alt.method.clone())).filter(|method| !method.is_empty())
}

/// The current work slice and the next (`STOCK_SLICES`), `level_of` the
/// bot's live levels.
pub fn work_slices(life: &Life, level_of: &dyn Fn(&str) -> u8) -> Vec<WorkSlice> {
    let active = |node: &crate::model::Node| node.status == NodeStatus::Active || life.active.as_deref() == Some(node.id.as_str());
    let mut order: Vec<&crate::model::Node> = life.nodes.iter().filter(|node| open(&node.status)).collect();
    order.sort_by_key(|node| !active(node));
    let mut out = Vec::new();
    for node in order {
        if out.len() >= STOCK_SLICES {
            break;
        }
        let slice = match &node.kind {
            NodeKind::ReachLevel { skill, level } => {
                let before = life
                    .nodes
                    .iter()
                    .filter(|other| other.id != node.id && open(&other.status))
                    .filter_map(|other| match &other.kind {
                        NodeKind::ReachLevel { skill: same, level: target } if same == skill && target < level => Some(*target),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(1);
                let from = level_of(skill).max(before).max(1);
                if from >= *level {
                    continue;
                }
                let method = node_method(node).unwrap_or_else(|| crate::tranches::reach_method(skill, from));
                WorkSlice { node: node.id.clone(), method, skill: Some(skill.clone()), from, to: *level, window_ticks: None }
            }
            NodeKind::MakeMoney => {
                let Some(method) = node_method(node) else { continue };
                let drawn = life.engagement.as_ref().filter(|engaged| engaged.node == node.id).map(|engaged| engaged.max_ticks as f64);
                let window = drawn.unwrap_or((crate::replan::ENGAGE_MIN + crate::replan::ENGAGE_MAX) as f64 / 2.0);
                WorkSlice { node: node.id.clone(), method, skill: None, from: 0, to: 0, window_ticks: Some(window) }
            }
            NodeKind::HaveCoins { .. } => {
                let Some(method) = node_method(node) else { continue };
                WorkSlice { node: node.id.clone(), method, skill: None, from: 0, to: 0, window_ticks: Some(HAVE_COINS_WINDOW_TICKS) }
            }
            _ => continue,
        };
        out.push(slice);
    }
    out
}

/// Work ticks a slice lasts: its window, else the XP of its tranche at
/// `xp_per_6000` (the method's derived XP in its skill). `None`: no
/// positive XP rate, no duration (no stock).
pub fn slice_ticks(slice: &WorkSlice, xp_per_6000: Option<f64>) -> Option<f64> {
    if let Some(window) = slice.window_ticks {
        return Some(window.max(0.0));
    }
    let rate = xp_per_6000.filter(|rate| rate.is_finite() && *rate > 0.0)?;
    Some(crate::sizing::xp_between(slice.from, slice.to) / rate * 6000.0)
}

/// Units consumed per 6000 ticks, per object (debug names), from a
/// yield's signed flows `(item, cause, per 6000)`; food eaten re-expressed
/// in `food` by hitpoints when given.
pub fn consumed_per_6000(flows: &[(String, String, f64)], food: Option<&str>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let food_heal = food.map(|name| (name, lifescape_gamedata::food::heal(name))).filter(|(_, heal)| *heal > 0);
    for (item, cause, qty) in flows {
        if *qty >= 0.0 || !qty.is_finite() || !CONSUMED_CAUSES.contains(&cause.as_str()) {
            continue;
        }
        let (name, units) = match food_heal {
            Some((chosen, heal)) if cause == "food" && lifescape_gamedata::food::heal(item) > 0 => (chosen.to_string(), -qty * lifescape_gamedata::food::heal(item) as f64 / heal as f64),
            _ => (item.clone(), -qty),
        };
        match out.iter_mut().find(|(known, _)| *known == name) {
            Some(row) => row.1 += units,
            None => out.push((name, units)),
        }
    }
    out
}

/// Target units per object over the slices `(consumed per 6000, ticks)`,
/// in slice order (the current slice's objects first).
pub fn slice_needs(slices: &[(Vec<(String, f64)>, f64)]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for (consumed, ticks) in slices {
        for (item, per_6000) in consumed {
            let units = per_6000 * ticks / 6000.0;
            if units.is_nan() || units <= 0.0 {
                continue;
            }
            match out.iter_mut().find(|(known, _)| known == item) {
                Some(row) => row.1 += units,
                None => out.push((item.clone(), units)),
            }
        }
    }
    out
}

/// Share of its target a bot buys ahead at `affinity` (D13): none under
/// `AUTARKY_BELOW`, the whole target at 1, linear between.
pub fn bought_share(affinity: f64) -> f64 {
    ((affinity.clamp(0.0, 1.0) - AUTARKY_BELOW) / (1.0 - AUTARKY_BELOW)).clamp(0.0, 1.0)
}

/// What the plan reads of the bot and the book: units held free (plus
/// bought in flight), units its resting STOCK bid still asks, the unit
/// ceiling of a stock bid (`None`: no public price, or not credible),
/// its market affinity and the coins the stock bids may commit.
pub struct StockView<'a> {
    pub held: &'a dyn Fn(&str) -> i64,
    pub resting: &'a dyn Fn(&str) -> i64,
    pub ceiling_of: &'a dyn Fn(&str) -> Option<i64>,
    pub market_affinity: f64,
    pub budget: i64,
}

/// What the brain does for one object.
#[derive(Debug, Clone, PartialEq)]
pub enum StockStep {
    /// A new stock bid for `qty` units at most `ceiling` each.
    Bid { qty: i64, ceiling: i64 },
    /// The resting stock bid stays (ceiling updated when priced).
    Keep { ceiling: Option<i64> },
    /// The holdings reach the target (a resting bid goes, `need_met`).
    Met,
    /// Over the refill point: no new bid.
    Wait,
    /// No public price: never bid.
    Unpriced,
    /// The stock budget is spent.
    Short,
}

/// One object of the stock plan: its target (after the affinity share),
/// what the bot holds, the step.
#[derive(Debug, Clone, PartialEq)]
pub struct StockLine {
    pub item: String,
    pub target: i64,
    pub held: i64,
    pub step: StockStep,
}

/// The stock plan of `needs` (slice order kept: the budget serves the
/// current slice first). An object whose share rounds to nothing has no
/// line (an autarkic bot: none at all).
pub fn stock_plan(needs: &[(String, f64)], view: &StockView) -> Vec<StockLine> {
    let share = bought_share(view.market_affinity);
    let mut budget = view.budget.max(0);
    let mut out = Vec::new();
    for (item, units) in needs {
        let target = (units * share).ceil() as i64;
        if target <= 0 {
            continue;
        }
        let held = (view.held)(item).max(0);
        let ceiling = (view.ceiling_of)(item).filter(|ceiling| *ceiling >= 1);
        let step = if held >= target {
            StockStep::Met
        } else if (view.resting)(item) > 0 {
            StockStep::Keep { ceiling }
        } else if (held as f64) > target as f64 * REFILL_AT {
            StockStep::Wait
        } else if let Some(ceiling) = ceiling {
            let qty = (target - held).min(budget / ceiling);
            if qty < 1 {
                StockStep::Short
            } else {
                budget -= qty * ceiling;
                StockStep::Bid { qty, ceiling }
            }
        } else {
            StockStep::Unpriced
        };
        out.push(StockLine { item: item.clone(), target, held, step });
    }
    out
}

/// Why a resting stock bid on `item` goes under `plan`: `need_met` when
/// its line is met, `need_gone` when no line wants it; `None`: it stays.
pub fn withdrawal(item: &str, plan: &[StockLine]) -> Option<&'static str> {
    match plan.iter().find(|line| line.item == item) {
        None => Some(NEED_GONE),
        Some(line) if line.step == StockStep::Met => Some(NEED_MET),
        Some(_) => None,
    }
}

#[cfg(test)]
#[path = "stock_target_tests.rs"]
mod tests;
