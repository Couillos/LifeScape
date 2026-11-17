#![forbid(unsafe_code)]
//! Discovery of a NEW object in the lifecycle simulator (plan marché
//! M7): five sellers whose own costs are 20-40, five buyers whose use
//! values are 30-90, arriving over a short span, on a book nobody ever
//! traded. The public anchors are what the daemon reads: the general
//! store's offer, the production cost the guides publish (no shop sells
//! the object). Criteria: the first fill lands inside [20, 90] within
//! three concession windows of the moment both sides are present, no
//! sale ever under the shop's offer, with or without a market maker.
//! Every seller has the Q3 outlet like in the daemon: a lot the market
//! pays less for than the general store goes to the store.

use crate::concession::{window_ticks, Urgency};
use crate::discovery::Anchors;
use crate::sim::FixedRng;
use crate::sim_lifecycle::{run_full, MakerRun, Model, SimFill, SimMaker, Trader};

/// Horizon of every order of the scenario (surplus / gear motives).
pub const HORIZON_TICKS: u64 = 36_000;
/// Arrivals are drawn over this span.
pub const SPREAD_TICKS: u64 = 6_000;
/// What the general store pays for one unit (Q3 outside option).
pub const SHOP_SALE: i64 = 25;
/// The production cost the guides publish.
pub const MAKE_COST: i64 = 30;
/// The object's game value (the maker's shop anchor).
pub const GAME_VALUE: i64 = 60;

/// The scenario's public anchors.
pub fn anchors() -> Anchors {
    Anchors { shop_sale: Some(SHOP_SALE), alch_net: None, make_cost: Some(MAKE_COST), shop_buy: None }
}

/// Five sellers (costs 20-40, 5 units) and five buyers (values 30-90,
/// 3 units), patience 0.05-0.95, arrivals over `SPREAD_TICKS`.
pub fn new_object_market(seed: u64) -> Vec<Trader> {
    let mut rng = FixedRng::new(seed);
    let mut traders = Vec::new();
    for index in 0..10 {
        let sell = index >= 5;
        let limit = if sell { rng.between(20, 40) } else { rng.between(30, 90) };
        traders.push(Trader {
            name: format!("{}{index}", if sell { "seller" } else { "buyer" }),
            sell,
            limit,
            units: if sell { 5 } else { 3 },
            patience: rng.between(5, 95) as f64 / 100.0,
            urgency: Urgency::Normal,
            horizon_ticks: HORIZON_TICKS,
            arrives: rng.between(0, SPREAD_TICKS as i64) as u64,
            loot: false,
            anchor: None,
        });
    }
    traders
}

/// The scenario's maker: wave-3 caps, the shop's offer as outside
/// option, the published cost as discovery anchor.
pub fn discovery_maker() -> SimMaker {
    SimMaker { make_cost: Some(MAKE_COST), ..SimMaker::standard(100_000, GAME_VALUE, Some(SHOP_SALE)) }
}

/// One run of the scenario (`anchors`: `None` for the openings of
/// wave 2, the comparison).
pub fn run(seed: u64, maker: bool, anchors: Option<Anchors>) -> (Vec<Trader>, MakerRun) {
    let traders = new_object_market(seed);
    let maker = maker.then(discovery_maker);
    let run = run_full(&traders, Model::Concession, SPREAD_TICKS + HORIZON_TICKS, maker, Some(SHOP_SALE), anchors);
    (traders, run)
}

/// The tick both sides first meet. With the maker it bids from its
/// first pass (tick 0): the later of the first buyer's and the first
/// seller's arrival. Without it, a seller that finds no bid at all takes
/// the shop's offer (Q3): the first seller arriving once a buyer is
/// there (`None`: every seller came before any buyer).
pub fn sides_meet_at(traders: &[Trader], maker: bool) -> Option<u64> {
    let first_buyer = traders.iter().filter(|trader| !trader.sell).map(|trader| trader.arrives).min()?;
    let sellers =traders.iter().filter(|trader| trader.sell).map(|trader| trader.arrives);
    if maker {
        return sellers.min().map(|seller| seller.max(first_buyer));
    }
    sellers.filter(|arrives| *arrives >= first_buyer).min()
}

/// The first print and how many concession windows after `met` it came
/// (rounded up; 0 when it came at once).
pub fn first_fill(fills: &[SimFill], met: u64) -> Option<(SimFill, u64)> {
    let first = fills.first()?.clone();
    let window = window_ticks(HORIZON_TICKS).max(1);
    let waited = first.tick.saturating_sub(met);
    Some((first, waited.div_ceil(window)))
}

#[cfg(test)]
#[path = "sim_discovery_tests.rs"]
mod tests;
