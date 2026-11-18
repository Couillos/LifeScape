//! Derived demand (user requirement 2026-11-14: a strong demand for a
//! product pulls its components' prices up, because producers want to buy
//! them to make it). A producer committed to a plan (its graph makes a
//! product the market values) prices each input it obtains from what the
//! product is worth to it, like a player who knows a helm sells 123 and
//! needs two bars: it pays up to `(value × (1 − margin) − its own time
//! per unit − the other inputs) / bars per helm` for a bar.
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
use crate::model::{Node, NodeKind};

/// Share of the product's value the producer keeps as its margin.
pub const DERIVED_MARGIN_PCT: f64 = 20.0;
/// The standing bid opens at this share of the reservation price when the
/// book is cheaper.
pub const DERIVED_OPEN_PCT: i64 = 50;
/// Consumers followed up the graph (helm ← bar ← ore ← …).
const MAX_DERIVED_DEPTH: u8 = 4;

/// What the reservation price reads (public values, the bot's own rates).
pub struct DerivedPrices<'a> {
    /// Market value of one unit of a product: others' best bid, else the
    /// current price (`None`: no value known).
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
        let reserve = (value * (1.0 - DERIVED_MARGIN_PCT / 100.0) - time - others) / share;
        if best.is_none_or(|known| reserve > known) {
            best = Some(reserve);
        }
    }
    best
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
