#![forbid(unsafe_code)]
//! Daemon side of the grouped auction (lot 3 marché, B2/B3; the pure
//! math is `lifescape_market::auction`).
//!
//! Every `AUCTION_PERIOD_TICKS` (`auction_round`, from the convene loop):
//! 1. B3: each object with resting orders (and each auction object) is
//!    measured on its own history, the mean time between its newest
//!    trades (`Store::fill_clocks`), and put in auction mode or back in
//!    continuous mode with the hysteresis of `auction::next_mode`; its
//!    book switches (`Book::set_batch`): no match on arrival, no uncross;
//! 2. B2: each auction object clears once, at the single price that
//!    trades the most units (`MarketService::auction_books`), the
//!    deals executing like any other match;
//! 3. what an auction left resting walks ONCE (`walk_items`, read by
//!    `market_reprice::reprice_round`): the 10 % walk plays only after a
//!    missed auction (a seller without a buyer or the reverse, or prices
//!    that did not meet).
//!
//! The mode lives in the engine's memory: after a restart the first
//! round re-measures every object from the executions.

use std::collections::HashSet;

use lifescape_market::auction::{self, AUCTION_PERIOD_TICKS, GAP_SAMPLE_FILLS};
use lifescape_market::Side;
use lifescape_store::prices::SettledFill;

use crate::market_service::MarketService;

/// Auction state of the daemon's market.
#[derive(Debug, Default)]
pub struct AuctionState {
    /// Objects in auction mode.
    pub items: HashSet<i64>,
    /// Daemon wall seconds of the last round (`None`: not yet run).
    pub last_run_wall: Option<f64>,
    /// Auction objects whose unserved orders walk at the next repricing
    /// pass (set by a round, taken by `reprice_round`).
    pub walk_items: HashSet<i64>,
}

/// What one auction round did (log and tests).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AuctionReport {
    /// Objects in auction mode after the round.
    pub auctioned: usize,
    /// Objects that entered / left the auction mode this round.
    pub entered: usize,
    pub left: usize,
    /// Deals opened by the auctions.
    pub deals: usize,
}

/// Wall seconds between two auctions at the measured tick.
pub fn period_secs(tick_ms: f64) -> f64 {
    AUCTION_PERIOD_TICKS as f64 * tick_ms.max(1.0) / 1000.0
}

/// Recent settled trades of `item` (B4: the last `ANCHOR_WINDOW_TICKS`,
/// at most `ANCHOR_MAX_FILLS`), parties included so the asker's own can
/// be left out.
pub fn recent_fills(service: &MarketService, item: i64, tick_ms: f64) -> Vec<SettledFill> {
    let window_secs = (auction::ANCHOR_WINDOW_TICKS as f64 * tick_ms.max(1.0) / 1000.0) as i64;
    let since = MarketService::now_clock() - window_secs;
    service.lock_store().settled_fills_since(item, since, auction::ANCHOR_MAX_FILLS).unwrap_or_default()
}

/// B4: the volume-weighted mean price of `fills` without those of
/// `account` (its own fills never anchor its own quote).
pub fn anchor_for(fills: &[SettledFill], account: &str) -> Option<i64> {
    let others: Vec<(i64, i64)> = fills.iter().filter(|fill| fill.buyer != account && fill.seller != account).map(|fill| (fill.price, fill.quantity)).collect();
    auction::anchor_price(&others)
}

/// True when `item` is in auction mode.
pub fn in_auction(service: &MarketService, item: i64) -> bool {
    service.lock_engine().auction.items.contains(&item)
}

/// One auction round at daemon wall `wall` (a no-op until a period has
/// run since the last one).
pub fn auction_round(service: &MarketService, wall: f64, tick_ms: f64) -> AuctionReport {
    let mut report = AuctionReport::default();
    {
        let mut engine = service.lock_engine();
        if engine.auction.last_run_wall.is_some_and(|last| wall - last < period_secs(tick_ms)) {
            return report;
        }
        engine.auction.last_run_wall = Some(wall);
    }
    // B3: the objects to measure, then their gaps (store reads, no
    // engine lock held).
    let (candidates, was): (Vec<i64>, HashSet<i64>) = {
        let engine = service.lock_engine();
        let mut items: HashSet<i64> = engine.auction.items.clone();
        items.extend(engine.books.iter().filter(|(_, book)| book.resting_qty(Side::Buy) + book.resting_qty(Side::Sell) > 0).map(|(item, _)| *item));
        let mut sorted: Vec<i64> = items.into_iter().collect();
        sorted.sort_unstable();
        (sorted, engine.auction.items.clone())
    };
    let now = MarketService::now_clock();
    let gaps: Vec<(i64, Option<f64>)> = candidates
        .iter()
        .map(|item| {
            let clocks = service.lock_store().fill_clocks(*item, GAP_SAMPLE_FILLS).unwrap_or_default();
            (*item, auction::trade_gap_ticks(&clocks, now, tick_ms))
        })
        .collect();
    let auctioned: Vec<i64> = {
        let mut engine = service.lock_engine();
        for (item, gap) in &gaps {
            let mode = auction::next_mode(was.contains(item), *gap);
            if mode && !was.contains(item) {
                report.entered += 1;
            }
            if !mode && was.contains(item) {
                report.left += 1;
            }
            if mode {
                engine.auction.items.insert(*item);
            } else {
                engine.auction.items.remove(item);
            }
            if let Some(book) = engine.books.get_mut(item) {
                book.set_batch(mode);
            }
        }
        let mut items: Vec<i64> = engine.auction.items.iter().copied().collect();
        items.sort_unstable();
        items
    };
    report.auctioned = auctioned.len();
    // B2: one clearing price per auction object.
    report.deals = service.auction_books(&auctioned, wall).len();
    // The orders left resting walk once (B2: after a missed auction).
    let leftover: HashSet<i64> = {
        let engine = service.lock_engine();
        auctioned.iter().copied().filter(|item| engine.books.get(item).is_some_and(|book| book.resting_qty(Side::Buy) + book.resting_qty(Side::Sell) > 0)).collect()
    };
    service.lock_engine().auction.walk_items = leftover;
    if report.entered + report.left + report.deals > 0 {
        crate::logln!("market-auction: {} object(s) in auction ({} entered, {} left), {} deal(s) at a single price", report.auctioned, report.entered, report.left, report.deals);
    }
    report
}

#[cfg(test)]
#[path = "market_auction_tests.rs"]
mod tests;
