//! Derived demand (user requirement 2026-11-14: a strong demand for a
//! product pulls its components' prices up, because producers want to buy
//! them to make it). A producer committed to a plan (its graph makes a
//! product the market values) prices each input it obtains from what the
//! product is worth to it, like a player who knows a helm sells 123 and
//! needs two bars: it pays up to `(value − its own time per unit − the
//! other inputs) / bars per helm` for a bar.
//!
//! Plan équilibre step 4 (live 2026-11-19: coal bid 17 under asks at 27,
//! gold bars bid 1 under 110, inputs resting at 1 coin by the million):
//! - value = what the product sells for on the SHARED demand
//!   (`product_value`: others' bids and a day's sales at the current
//!   price, less the supply on its way, the asks resting under it and the
//!   units other producers claimed, spread over the lot), never a stale
//!   current price nor a bid others already serve;
//! - time = the bot's REAL time value (step 1, `brain_d17::coin_rate`,
//!   measured gain per work tick);
//! - no private margin on top (the time value already pays the bot): an
//!   input under its transformation value is bid over the ask, the
//!   transformer buys and makes, the input's price climbs.
//!
//! The reservation price caps the standing bid placed beside the input's
//! production (D-B, `brain_market::place_standing_bid`) together with the
//! wallet, and opens it at `DERIVED_OPEN_PCT` of the reservation when the
//! book is cheaper (a 2-coin bar bid nobody fills never moves a miner).
//! Smelters and miners answer those bids through the same engine
//! (`bid_response`): bars, ores, coal, any input of any F2P recipe.
//!
//! Recursive: an ore feeding a bar feeding a helm is worth the bar's
//! derived value, smelting time and coal paid. Pure: the graph, the
//! public values and the bot's own rates come in as arguments.

use crate::bid_response::per_unit_inputs;
use crate::market_fit::BookView;
use crate::market_value::{PENDING_SUPPLY_PCT, SALES_SHARE_PCT};
use crate::model::{GoalKind, Node, NodeKind, NodeStatus};

/// The standing bid opens at this share of the reservation price when the
/// book is cheaper.
pub const DERIVED_OPEN_PCT: i64 = 50;
/// Consumers followed up the graph (helm ← bar ← ore ← …).
const MAX_DERIVED_DEPTH: u8 = 4;

/// What the reservation price reads (public values, the bot's own rates).
pub struct DerivedPrices<'a> {
    /// What one unit of a product sells for on the shared demand
    /// (`product_value`; `None`: no value known).
    pub value_of: &'a dyn Fn(&str) -> Option<f64>,
    /// Coins one unit of another input of the recipe costs (best ask,
    /// else current price).
    pub input_price: &'a dyn Fn(&str) -> f64,
    /// Units per 6000 ticks of a method for an object (learned rates,
    /// else the prior).
    pub units_per_6000: &'a dyn Fn(&str, &str) -> f64,
    pub coin_rate: f64,
}

/// Reservation price of one unit of what the obtain node at `index`
/// gets, derived from the products its consumers in the graph make (the
/// best of them); `None` when it feeds no valued product.
pub fn input_reserve(nodes: &[Node], index: usize, prices: &DerivedPrices) -> Option<f64> {
    reserve_at(nodes, index, prices, 0)
}

/// The reservation of the brain's bids on an input (`brain_market::
/// derived_reserve`) in a life whose goal is `goal`: `input_reserve`
/// when the input is made into a product FOR SALE (`made_for_sale`),
/// `None` otherwise (review lot 4, point 3): a product the bot uses
/// (gear, a tool of its kit, a quest item) is never valued at its sale on
/// the shared demand, its inputs keep the wallet as their ceiling.
pub fn sale_reserve(nodes: &[Node], index: usize, goal: &GoalKind, prices: &DerivedPrices) -> Option<f64> {
    if !made_for_sale(nodes, index, goal) {
        return None;
    }
    input_reserve(nodes, index, prices)
}

/// True when the obtain node at `index` ends, through every consumer up
/// the graph, in the life's `Produce` quota (an ambition milestone or a
/// bid answer: the default selling lists it), each step an input of its
/// consumer's recipe. Any other goal (gear, a kit tool, a quest item, an
/// object obtained for itself), or a step the consumer uses as a tool,
/// is the bot's own use.
pub fn made_for_sale(nodes: &[Node], index: usize, goal: &GoalKind) -> bool {
    let GoalKind::Produce { item: quota, .. } = goal else { return false };
    sold_through(nodes, index, quota, 0)
}

fn sold_through(nodes: &[Node], index: usize, quota: &str, depth: u8) -> bool {
    let Some(node) = nodes.get(index) else { return false };
    let NodeKind::ObtainItems { item, .. } = &node.kind else { return false };
    let consumers: Vec<(usize, &Node)> = nodes.iter().enumerate().filter(|(_, other)| other.deps.contains(&node.id) && matches!(other.kind, NodeKind::ObtainItems { .. })).collect();
    if consumers.is_empty() {
        return item == quota;
    }
    if depth >= MAX_DERIVED_DEPTH {
        return false;
    }
    consumers.into_iter().all(|(consumer_index, consumer)| {
        let NodeKind::ObtainItems { item: product, .. } = &consumer.kind else { return false };
        let input_of_recipe = crate::obtain_legs::production_legs(product).iter().any(|leg| leg.inputs.iter().any(|(name, _)| name == item));
        input_of_recipe && sold_through(nodes, consumer_index, quota, depth + 1)
    })
}

fn reserve_at(nodes: &[Node], index: usize, prices: &DerivedPrices, depth: u8) -> Option<f64> {
    if depth >= MAX_DERIVED_DEPTH {
        return None;
    }
    let node = nodes.get(index)?;
    let NodeKind::ObtainItems { item: input, .. } = &node.kind else { return None };
    let mut best: Option<f64> = None;
    for (consumer_index, consumer) in nodes.iter().enumerate().filter(|(_, other)| other.deps.contains(&node.id)) {
        let NodeKind::ObtainItems { item: product, .. } = &consumer.kind else { continue };
        let legs = crate::obtain_legs::production_legs(product);
        let consumes = |leg: &&crate::obtain_legs::ProductionLeg| leg.inputs.iter().any(|(name, _)| name == input);
        let leg = legs.iter().filter(consumes).find(|leg| consumer.method.as_deref() == Some(leg.method.as_str())).or_else(|| legs.iter().find(consumes));
        let Some(leg) = leg else { continue };
        let value = match ((prices.value_of)(product), reserve_at(nodes, consumer_index, prices, depth + 1)) {
            (Some(market), Some(derived)) => market.max(derived),
            (market, derived) => match market.or(derived) {
                Some(value) => value,
                None => continue,
            },
        };
        let per_unit = per_unit_inputs(&leg.method, &leg.inputs);
        let Some(share) = per_unit.iter().find(|(name, _)| name == input).map(|(_, qty)| *qty).filter(|qty| *qty > 0.0) else { continue };
        let others: f64 = per_unit.iter().filter(|(name, _)| name != input).map(|(name, qty)| qty * (prices.input_price)(name).max(0.0)).sum();
        let rate = (prices.units_per_6000)(&leg.method, product);
        if !rate.is_finite() || rate <= 0.0 {
            continue;
        }
        let time = 6000.0 / rate * prices.coin_rate;
        let reserve = (value - time - others) / share;
        if best.is_none_or(|known| reserve > known) {
            best = Some(reserve);
        }
    }
    best
}

/// What one unit of a product in a lot of `units` sells for on the
/// SHARED demand (plan équilibre steps 2 and 4; the brain's `value_of`):
/// the public book (others' orders), best prices first:
/// - others' bids, less the supply on its way (asks within
///   `PENDING_SUPPLY_PCT` of the best bid walk down into the best bids);
/// - a day's sales share at the current price (`SALES_SHARE_PCT` of the
///   24 h volume) behind the asks already resting at or under it;
/// - the units other producers claimed come first (they take the best
///   prices, as `guide_engine::net_ahead` takes the supply ahead off the
///   best steps);
/// - the NPC shop and alchemy outlets are left out (conservative: they
///   never raise an input's ceiling);
///
/// summed over the lot and spread over its `units`: a unit the market no
/// longer takes is worth nothing to the inputs bought for it. 0 when no
/// demand is left (a stale current price with no volume, every bid
/// claimed); `None` when the book knows no price at all (never traded,
/// no bid).
pub fn product_value(view: Option<&BookView>, units: i64) -> Option<f64> {
    let view = view?;
    if view.best_bid().is_none() && view.current.is_none() {
        return None;
    }
    let units = units.max(1);
    let mut bids: Vec<(i64, i64)> = view.bids.iter().copied().filter(|(price, qty)| *price > 0 && *qty > 0).collect();
    bids.sort_by(|left, right| right.0.cmp(&left.0));
    let pending = view.best_bid().map(|best| view.asks_at_or_under(best.saturating_mul(PENDING_SUPPLY_PCT) / 100)).unwrap_or(0);
    take_from_top(&mut bids, pending);
    let mut ladder = bids;
    if let Some(price) = view.current.filter(|price| *price > 0) {
        let sales = view.volume_24h.max(0) * SALES_SHARE_PCT / 100 - view.asks_at_or_under(price);
        if sales > 0 {
            ladder.push((price, sales));
        }
    }
    ladder.sort_by(|left, right| right.0.cmp(&left.0));
    take_from_top(&mut ladder, view.claimed);
    let mut left = units;
    let mut revenue = 0.0;
    for (price, qty) in ladder {
        if left <= 0 {
            break;
        }
        let taken = qty.min(left);
        revenue += (price * taken) as f64;
        left -= taken;
    }
    Some(revenue / units as f64)
}

/// Removes `units` from the top of a (price, units) ladder, best first.
fn take_from_top(ladder: &mut Vec<(i64, i64)>, units: i64) {
    let mut left = units.max(0);
    for step in ladder.iter_mut() {
        let taken = step.1.min(left);
        step.1 -= taken;
        left -= taken;
    }
    ladder.retain(|(_, qty)| *qty > 0);
}

/// Units of `product` the graph makes: the largest count of an open
/// obtain node of it (1 at least).
pub fn product_lot(nodes: &[Node], product: &str) -> i64 {
    nodes
        .iter()
        .filter(|node| !matches!(node.status, NodeStatus::Done | NodeStatus::Abandoned))
        .filter_map(|node| match &node.kind {
            NodeKind::ObtainItems { item, count } if item == product => Some(*count),
            _ => None,
        })
        .max()
        .unwrap_or(1)
        .max(1)
}

/// Bounds of a standing input bid with a reservation price: the ceiling
/// is the wallet's (`wallet_ceiling`) capped by the reservation (at least
/// one coin: a product worth less than its inputs never buys them high),
/// the opening at least `DERIVED_OPEN_PCT` of the reservation (never
/// above the ceiling). Returns (opening floor, ceiling).
pub fn derived_bounds(reserve: f64, wallet_ceiling: i64) -> (i64, i64) {
    let ceiling = wallet_ceiling.min(reserve.floor().max(1.0) as i64).max(1);
    let open = ((reserve.max(0.0) * DERIVED_OPEN_PCT as f64 / 100.0).ceil() as i64).clamp(1, ceiling);
    (open, ceiling)
}

/// Ceiling of a `market:buy` bid on an input (live 2026-11-18: 31
/// sapphire cutters bid up to 1 092 coins an uncut sapphire for a
/// sapphire worth 175, the wallet their only limit): the wallet's,
/// capped by the reservation price when the object feeds a product
/// (`derived_bounds`); the wallet alone otherwise (a purchase for itself).
pub fn obtain_ceiling(wallet_ceiling: i64, reserve: Option<f64>) -> i64 {
    match reserve {
        Some(reserve) => derived_bounds(reserve, wallet_ceiling).1,
        None => wallet_ceiling.max(1),
    }
}

#[cfg(test)]
#[path = "derived_demand_tests.rs"]
mod tests;
