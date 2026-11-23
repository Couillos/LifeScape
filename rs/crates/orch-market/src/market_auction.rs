#![forbid(unsafe_code)]
//! Daemon side of the grouped auction (lot 3 marché, B2/B3; the pure
//! math is `lifescape_market::auction`).
//!
//! Every `AUCTION_PERIOD_TICKS` (`auction_round`, from the convene loop):
//! 1. B3: each object with resting orders (and each auction object) is
//!    put in auction mode or back in continuous mode with the hysteresis
//!    of `auction::next_mode`, on the SAME quantity in both modes, the
//!    mean time between two trades: measured on the executions for a
//!    continuous object, as the period over the pairs served per auction
//!    for an auction object (`auction::equivalent_gap_ticks`: it trades
//!    once per auction, so its executions alone could never show it
//!    brisk again). The book switches (`Book::set_batch`): no match on
//!    arrival, no uncross;
//! 2. B2: each auction object clears once, at the single price that
//!    trades the most units (`MarketService::auction_books`), the
//!    deals executing like any other match;
//! 3. what an auction left resting walks ONCE (`walk_items`, read by
//!    `market_reprice::reprice_round`): the 5 % walk plays only after a
//!    missed auction (a seller without a buyer or the reverse, or prices
//!    that did not meet).
//!
//! Between rounds: the first order of an object the engine has not
//! measured yet (`ensure_mode`, called by `place`) fixes its mode
//! before it can match: an object traded fewer than twice is in auction
//! mode from its first order. A PRESSED order (a blocking `need`, an
//! `urgent` sale: full bank) posted on an auction object triggers an
//! auction on it at once (`pressed_auction`), same single price for
//! every order present; it never waits for the period.
//!
//! The mode lives in the engine's memory: after a restart the first
//! round (or the first order) re-measures every object.

use std::collections::{HashMap, HashSet, VecDeque};

use lifescape_market::auction::{self, AUCTION_HISTORY, AUCTION_PERIOD_TICKS, GAP_SAMPLE_FILLS};
use lifescape_market::{Book, Side};

use crate::market_service::MarketService;

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Test thread opted in to the first-order mode (`ensure_mode`).
    static FIRST_ORDER_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test helper: this thread's services fix an unmeasured object's mode
/// at its first order, as the daemon does.
#[cfg(any(test, feature = "test-support"))]
pub fn enforce_first_order_mode() {
    FIRST_ORDER_MODE.with(|enforced| enforced.set(true));
}

/// Auction state of the daemon's market.
#[derive(Debug, Default)]
pub struct AuctionState {
    /// Objects in auction mode.
    pub items: HashSet<i64>,
    /// Objects whose mode was measured (a round or a first order).
    pub known: HashSet<i64>,
    /// Daemon wall seconds of the last round (`None`: not yet run).
    pub last_run_wall: Option<f64>,
    /// Auction objects whose unserved orders walk at the next repricing
    /// pass (set by a round, taken by `reprice_round`).
    pub walk_items: HashSet<i64>,
    /// Pairs served per object since the last round (immediate auctions
    /// of pressed orders count toward the period they fall in).
    pub pairs_since: HashMap<i64, u32>,
    /// Pairs served per auction, newest last, `AUCTION_HISTORY` kept.
    pub history: HashMap<i64, VecDeque<u32>>,
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

/// B4: the volume-weighted mean price of `item`'s settled trades of the
/// last `ANCHOR_WINDOW_TICKS` (at most `ANCHOR_MAX_FILLS`), WITHOUT
/// those of `account` (its own fills never anchor its own quote: the
/// exclusion is in the query, the limit counts the others). A store
/// error is logged and the quote walks without an anchor this window.
pub fn anchor_of(service: &MarketService, item: i64, account: &str, tick_ms: f64) -> Option<i64> {
    let window_secs = (auction::ANCHOR_WINDOW_TICKS as f64 * tick_ms.max(1.0) / 1000.0) as i64;
    let since = MarketService::now_clock() - window_secs;
    let fills = service.lock_store().settled_fills_since(item, since, account, auction::ANCHOR_MAX_FILLS);
    match fills {
        Ok(fills) => auction::anchor_price(&fills.iter().map(|fill| (fill.price, fill.quantity)).collect::<Vec<_>>()),
        Err(error) => {
            crate::logln!("market-auction: anchor of object {item} for {account} unreadable, no anchor this window: {error}");
            None
        }
    }
}

/// True when `item` is in auction mode.
pub fn in_auction(service: &MarketService, item: i64) -> bool {
    service.lock_engine().auction.items.contains(&item)
}

/// Mode of `item` from its executions alone (a continuous object, a
/// first order): `None` and a log when the store cannot be read.
fn measured_mode(service: &MarketService, item: i64, was_auction: bool, tick_ms: f64) -> Option<bool> {
    let clocks = match service.lock_store().fill_clocks(item, GAP_SAMPLE_FILLS) {
        Ok(clocks) => clocks,
        Err(error) => {
            crate::logln!("market-auction: executions of object {item} unreadable, mode kept: {error}");
            return None;
        }
    };
    Some(auction::next_mode(was_auction, auction::trade_gap_ticks(&clocks, MarketService::now_clock(), tick_ms)))
}

/// First sight of an object (its first order, or the first after a
/// restart): its mode is fixed BEFORE the order can match. Traded fewer
/// than twice, it is an auction object from that order on.
pub fn ensure_mode(service: &MarketService, item: i64) {
    if service.lock_engine().auction.known.contains(&item) {
        return;
    }
    // Tests of the plain book (hundreds written before the auction)
    // keep the continuous first order; the auction tests opt in.
    #[cfg(any(test, feature = "test-support"))]
    if !FIRST_ORDER_MODE.with(|enforced| enforced.get()) {
        service.lock_engine().auction.known.insert(item);
        return;
    }
    let tick_ms = crate::market_eta::live_tick_ms().max(1) as f64;
    let Some(mode) = measured_mode(service, item, false, tick_ms) else { return };
    let mut engine = service.lock_engine();
    engine.auction.known.insert(item);
    if mode {
        engine.auction.items.insert(item);
    }
    engine.books.entry(item).or_insert_with(Book::new).set_batch(mode);
}

impl MarketService {
    /// Review lot 3 (2): a pressed order just posted on an auction
    /// object (`need` motive or `urgent`) triggers an auction on it at
    /// once: same clearing price for every order present. Returns the
    /// deals opened (empty off auction or when nothing crosses).
    pub(crate) fn pressed_auction(&self, item: i64, motive: &str, urgent: bool) -> Vec<i64> {
        if motive != crate::market_lineage::NEED && !urgent {
            return Vec::new();
        }
        if !in_auction(self, item) {
            return Vec::new();
        }
        let served = self.auction_books(&[item], crate::market_convene::wall_now());
        let deals: Vec<i64> = served.iter().map(|(_, deal)| *deal).collect();
        if !deals.is_empty() {
            *self.lock_engine().auction.pairs_since.entry(item).or_insert(0) += deals.len() as u32;
        }
        deals
    }
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
    // B3: the objects to measure.
    let (candidates, was): (Vec<i64>, HashSet<i64>) = {
        let engine = service.lock_engine();
        let mut items: HashSet<i64> = engine.auction.items.clone();
        items.extend(engine.books.iter().filter(|(_, book)| book.resting_qty(Side::Buy) + book.resting_qty(Side::Sell) > 0).map(|(item, _)| *item));
        let mut sorted: Vec<i64> = items.into_iter().collect();
        sorted.sort_unstable();
        (sorted, engine.auction.items.clone())
    };
    // An auction object with enough auctions behind it is measured on
    // the pairs it served; any other on its executions.
    let modes: Vec<(i64, bool)> = candidates
        .iter()
        .filter_map(|item| {
            let was_auction = was.contains(item);
            let history: Vec<u32> = service.lock_engine().auction.history.get(item).map(|kept| kept.iter().copied().collect()).unwrap_or_default();
            let equivalent = if was_auction { auction::equivalent_gap_ticks(&history) } else { None };
            let mode = match equivalent {
                Some(gap) => auction::next_mode(true, Some(gap)),
                None => measured_mode(service, *item, was_auction, tick_ms)?,
            };
            Some((*item, mode))
        })
        .collect();
    let auctioned: Vec<i64> = {
        let mut engine = service.lock_engine();
        for (item, mode) in &modes {
            let was_auction = was.contains(item);
            if *mode && !was_auction {
                report.entered += 1;
            }
            if !*mode && was_auction {
                report.left += 1;
            }
            if *mode != was_auction {
                engine.auction.history.remove(item);
                engine.auction.pairs_since.remove(item);
            }
            engine.auction.known.insert(*item);
            if *mode {
                engine.auction.items.insert(*item);
            } else {
                engine.auction.items.remove(item);
            }
            if let Some(book) = engine.books.get_mut(item) {
                book.set_batch(*mode);
            }
        }
        let mut items: Vec<i64> = engine.auction.items.iter().copied().collect();
        items.sort_unstable();
        items
    };
    report.auctioned = auctioned.len();
    // B2: one clearing price per auction object.
    let served = service.auction_books(&auctioned, wall);
    report.deals = served.len();
    {
        // Pairs of this period (immediate auctions included) become one
        // history entry per auction object.
        let mut engine = service.lock_engine();
        for item in &auctioned {
            let this_round = served.iter().filter(|(served_item, _)| served_item == item).count() as u32;
            let total = engine.auction.pairs_since.remove(item).unwrap_or(0) + this_round;
            let kept = engine.auction.history.entry(*item).or_default();
            kept.push_back(total);
            while kept.len() > AUCTION_HISTORY {
                kept.pop_front();
            }
        }
    }
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
