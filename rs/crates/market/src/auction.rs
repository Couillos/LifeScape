#![forbid(unsafe_code)]
//! Grouped auction of the rarely traded objects and the continuous
//! market's anchor (lot 3 marché, user plan B1-B5, 2026-11-21).
//!
//! Live 2026-11-21: 9 700 orders created and 9 000 cancelled an hour for
//! 689 trades; 21 % of the trades at one coin in 24 h. An object that
//! trades once in a long while was walked 10 % at a time against nobody:
//! each window moved prices on no information. Two regimes now:
//!
//! - continuous (frequent trades): the 10 % walk of `concession`, its
//!   reference the volume-weighted mean price of the recent trades
//!   (`anchor_price`, B4) and its step shrinking near it
//!   (`concession::step_pct`, B5);
//! - grouped auction (rare trades, B2): every `AUCTION_PERIOD_TICKS` the
//!   orders of the object clear at ONE price (`clearing`), the one that
//!   trades the most units; between two auctions the orders rest (the
//!   book does not match: `Book::set_batch`) and the 10 % walk plays
//!   only for what an auction left unserved.
//!
//! Mode (B3, `next_mode`): measured on the object's own history, the
//! mean time between two trades (`trade_gap_ticks`). Above one walk
//! window the object goes to auction; back to continuous only when its
//! trades are twice as frequent as the window (hysteresis: a gap between
//! the two thresholds keeps the current mode, no flip-flop). No raw count
//! threshold.

use crate::concession::MAX_WINDOW_TICKS;

/// Ticks between two auctions of an object in auction mode: 6 000 = three
/// walk windows at their slowest (2 000). An object enters auction mode
/// when it trades less often than once per window, so a period of three
/// windows gathers a few orders per side while a bot never waits more
/// than the old fill wait (6 000 ticks, the `need` horizon) for its
/// chance to trade.
pub const AUCTION_PERIOD_TICKS: u64 = 6_000;
/// Mean time between two trades above which an object goes to auction:
/// one walk window (the slowest, `MAX_WINDOW_TICKS`): the order would
/// walk a full step between two trades anyway.
pub const ENTER_GAP_TICKS: f64 = MAX_WINDOW_TICKS as f64;
/// Mean time between two trades under which an auction object returns
/// to continuous trading: twice as frequent as a window (the plan's
/// anti-flip margin).
pub const EXIT_GAP_TICKS: f64 = ENTER_GAP_TICKS / 2.0;
/// Newest fills the mean gap is measured on.
pub const GAP_SAMPLE_FILLS: i64 = 8;
/// Window of the volume-weighted anchor (B4), in ticks: 36 000, eighteen
/// slowest windows. A continuous object trades at least once per 2 000
/// ticks, so the anchor averages 18 trades or more and one isolated
/// one-coin sale hardly moves it.
pub const ANCHOR_WINDOW_TICKS: u64 = 36_000;
/// Most trades the anchor reads (daemon CPU).
pub const ANCHOR_MAX_FILLS: i64 = 50;

/// Outcome of the auction math: the single price and the units it trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clearing {
    pub price: i64,
    pub volume: i64,
    /// Lowest and highest order price that trade the maximum volume.
    pub lowest: i64,
    pub highest: i64,
}

fn volume_at(bids: &[(i64, i64)], asks: &[(i64, i64)], price: i64) -> i64 {
    let demand: i64 = bids.iter().filter(|(bid, _)| *bid >= price).map(|(_, quantity)| quantity).sum();
    let supply: i64 = asks.iter().filter(|(ask, _)| *ask <= price).map(|(_, quantity)| quantity).sum();
    demand.min(supply)
}

/// The single clearing price of `bids` and `asks` (price, units): the one
/// that trades the most units (buyers at or over it, sellers at or under
/// it). Ties: the volume is the same over an interval of prices; the
/// price is the MIDDLE of that interval (lowest and highest order price
/// reaching the maximum, rounded down), so neither side takes the whole
/// surplus. `None` when nothing crosses.
pub fn clearing(bids: &[(i64, i64)], asks: &[(i64, i64)]) -> Option<Clearing> {
    // Volume only changes at an order price: the maximum is at one.
    let mut prices: Vec<i64> = bids.iter().chain(asks.iter()).filter(|(price, quantity)| *price >= 1 && *quantity > 0).map(|(price, _)| *price).collect();
    prices.sort_unstable();
    prices.dedup();
    let best = prices.iter().map(|price| volume_at(bids, asks, *price)).max()?;
    if best <= 0 {
        return None;
    }
    let optimal: Vec<i64> = prices.iter().copied().filter(|price| volume_at(bids, asks, *price) == best).collect();
    let (lowest, highest) = (*optimal.first()?, *optimal.last()?);
    let middle = lowest + (highest - lowest) / 2;
    let price = if volume_at(bids, asks, middle) == best { middle } else { lowest };
    Some(Clearing { price, volume: best, lowest, highest })
}

/// Mean time between two trades of an object, in ticks, from the clocks
/// (store seconds) of its newest fills; `now` and `tick_ms` convert. The
/// larger of the mean gap between the sampled fills and the time since
/// the newest one: a burst followed by silence reads as rare. `None`
/// with fewer than two fills (nothing to average: rare by definition).
pub fn trade_gap_ticks(fill_clocks: &[i64], now: i64, tick_ms: f64) -> Option<f64> {
    if fill_clocks.len() < 2 {
        return None;
    }
    let newest = *fill_clocks.iter().max()?;
    let oldest = *fill_clocks.iter().min()?;
    let mean_secs = (newest - oldest) as f64 / (fill_clocks.len() - 1) as f64;
    let since_secs = (now - newest).max(0) as f64;
    Some(mean_secs.max(since_secs) * 1000.0 / tick_ms.max(1.0))
}

/// Next mode of an object (B3): `true` = grouped auction. Above
/// `ENTER_GAP_TICKS` (or no measurable gap) the auction; at or under
/// `EXIT_GAP_TICKS` continuous; in between the mode stays as it was.
pub fn next_mode(was_auction: bool, gap_ticks: Option<f64>) -> bool {
    match gap_ticks {
        None => true,
        Some(gap) if gap > ENTER_GAP_TICKS => true,
        Some(gap) if gap <= EXIT_GAP_TICKS => false,
        Some(_) => was_auction,
    }
}

/// Volume-weighted mean price of recent trades (price, units), rounded
/// to the nearest coin (B4); `None` without a trade.
pub fn anchor_price(fills: &[(i64, i64)]) -> Option<i64> {
    let (mut coins, mut units) = (0i128, 0i128);
    for (price, quantity) in fills.iter().filter(|(price, quantity)| *price >= 1 && *quantity > 0) {
        coins += i128::from(*price) * i128::from(*quantity);
        units += i128::from(*quantity);
    }
    (units > 0).then(|| ((coins + units / 2) / units).max(1) as i64)
}

#[cfg(test)]
#[path = "auction_tests.rs"]
mod tests;
