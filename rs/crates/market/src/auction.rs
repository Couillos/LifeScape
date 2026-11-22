#![forbid(unsafe_code)]
//! Grouped auction of the rarely traded objects and the continuous
//! market's anchor (lot 3 marché, user plan B1-B5, 2026-11-21).
//!
//! Live 2026-11-21: 9 700 orders created and 9 000 cancelled an hour for
//! 689 trades; 21 % of the trades at one coin in 24 h. An object that
//! trades once in a long while was walked 10 % at a time against nobody:
//! each window moved prices on no information. Two regimes now:
//!
//! - continuous (frequent trades): the 5 % walk of `concession`, its
//!   reference the volume-weighted mean price of the recent trades
//!   (`anchor_price`, B4) and its step shrinking near it
//!   (`concession::step_pct`, B5);
//! - grouped auction (rare trades, B2): every `AUCTION_PERIOD_TICKS` the
//!   orders of the object clear at ONE price (`clearing`), the one that
//!   trades the most units; between two auctions the orders rest (the
//!   book does not match: `Book::set_batch`) and the 5 % walk plays
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

/// One order of an auction (the book's priority order: best price first,
/// then oldest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuctionOrder {
    pub account: String,
    pub price: i64,
    pub quantity: i64,
}

/// Units the orders could trade at `price` if every bid at or over it
/// met every ask at or under it (an upper bound, used to skip prices).
fn unrestricted_volume(bids: &[AuctionOrder], asks: &[AuctionOrder], price: i64) -> i64 {
    let demand: i64 = bids.iter().filter(|order| order.price >= price).map(|order| order.quantity).sum();
    let supply: i64 = asks.iter().filter(|order| order.price <= price).map(|order| order.quantity).sum();
    demand.min(supply)
}

/// The pairs that trade at `price` (bid index, ask index, units): buyers
/// at or over it, sellers at or under it, only pairs `may_trade(buyer,
/// seller)` allows. A greedy pass in priority order (best price, oldest
/// first) then augmenting paths until no unit can be added: the volume
/// is the maximum the allowed pairs permit, whatever the order.
fn match_at(bids: &[AuctionOrder], asks: &[AuctionOrder], may_trade: &dyn Fn(&str, &str) -> bool, price: i64) -> Vec<(usize, usize, i64)> {
    let bid_ids: Vec<usize> = (0..bids.len()).filter(|index| bids[*index].price >= price && bids[*index].quantity > 0).collect();
    let ask_ids: Vec<usize> = (0..asks.len()).filter(|index| asks[*index].price <= price && asks[*index].quantity > 0).collect();
    let allowed: Vec<Vec<bool>> = bid_ids.iter().map(|b| ask_ids.iter().map(|a| may_trade(&bids[*b].account, &asks[*a].account)).collect()).collect();
    let mut flow = vec![vec![0i64; ask_ids.len()]; bid_ids.len()];
    let mut given = vec![0i64; bid_ids.len()];
    let mut taken = vec![0i64; ask_ids.len()];
    for b in 0..bid_ids.len() {
        for a in 0..ask_ids.len() {
            if !allowed[b][a] {
                continue;
            }
            let room = (bids[bid_ids[b]].quantity - given[b]).min(asks[ask_ids[a]].quantity - taken[a]);
            if room > 0 {
                flow[b][a] += room;
                given[b] += room;
                taken[a] += room;
            }
        }
    }
    // Augmenting paths: bid with units left -> allowed ask -> (ask full)
    // a bid already feeding it -> another ask ... -> ask with room.
    loop {
        let nodes = bid_ids.len() + ask_ids.len();
        let mut parent: Vec<Option<usize>> = vec![None; nodes];
        let mut seen = vec![false; nodes];
        let mut queue = std::collections::VecDeque::new();
        for b in 0..bid_ids.len() {
            if given[b] < bids[bid_ids[b]].quantity {
                seen[b] = true;
                queue.push_back(b);
            }
        }
        let mut sink = None;
        while let Some(node) = queue.pop_front() {
            if node < bid_ids.len() {
                for a in 0..ask_ids.len() {
                    let ask_node = bid_ids.len() + a;
                    if allowed[node][a] && !seen[ask_node] {
                        seen[ask_node] = true;
                        parent[ask_node] = Some(node);
                        queue.push_back(ask_node);
                    }
                }
            } else {
                let a = node - bid_ids.len();
                if taken[a] < asks[ask_ids[a]].quantity {
                    sink = Some(a);
                    break;
                }
                for b in 0..bid_ids.len() {
                    if flow[b][a] > 0 && !seen[b] {
                        seen[b] = true;
                        parent[b] = Some(node);
                        queue.push_back(b);
                    }
                }
            }
        }
        let Some(last_ask) = sink else { break };
        // Walk back to the source bid, then push the bottleneck.
        let mut path = vec![bid_ids.len() + last_ask];
        while let Some(before) = parent[*path.last().unwrap_or(&0)] {
            path.push(before);
        }
        path.reverse();
        let source = path[0];
        let mut push = (bids[bid_ids[source]].quantity - given[source]).min(asks[ask_ids[last_ask]].quantity - taken[last_ask]);
        for step in path.windows(2) {
            if step[0] >= bid_ids.len() {
                push = push.min(flow[step[1]][step[0] - bid_ids.len()]);
            }
        }
        if push <= 0 {
            break;
        }
        for step in path.windows(2) {
            if step[0] < bid_ids.len() {
                flow[step[0]][step[1] - bid_ids.len()] += push;
            } else {
                flow[step[1]][step[0] - bid_ids.len()] -= push;
            }
        }
        given[source] += push;
        taken[last_ask] += push;
    }
    let mut pairs = Vec::new();
    for b in 0..bid_ids.len() {
        for a in 0..ask_ids.len() {
            if flow[b][a] > 0 {
                pairs.push((bid_ids[b], ask_ids[a], flow[b][a]));
            }
        }
    }
    pairs
}

/// The single clearing price and the pairs that trade at it (review lot
/// 3: the equilibrium is computed on the pairs that may really trade,
/// `may_trade(buyer, seller)`: never an account with itself, never a
/// pair kept apart). The price is the one that trades the most units;
/// the volume is the same over an interval of prices and the price is
/// the MIDDLE of it (lowest and highest order price reaching the
/// maximum, rounded down) so neither side takes the whole surplus; when
/// the middle itself trades less (a restricted pair), the optimal order
/// price nearest to it, the lower on a tie. `bids` and `asks` come in
/// priority order. `None` when nothing can trade.
pub fn clear_pairs(bids: &[AuctionOrder], asks: &[AuctionOrder], may_trade: &dyn Fn(&str, &str) -> bool) -> Option<(Clearing, Vec<(usize, usize, i64)>)> {
    // Volume only changes at an order price: the maximum is at one.
    let mut prices: Vec<i64> = bids.iter().chain(asks.iter()).filter(|order| order.price >= 1 && order.quantity > 0).map(|order| order.price).collect();
    prices.sort_unstable();
    prices.dedup();
    let volume = |price: i64| match_at(bids, asks, may_trade, price).iter().map(|(_, _, units)| units).sum::<i64>();
    let mut best = 0;
    let mut optimal: Vec<i64> = Vec::new();
    for price in prices {
        if unrestricted_volume(bids, asks, price) < best.max(1) {
            continue;
        }
        let traded = volume(price);
        if traded > best {
            best = traded;
            optimal.clear();
        }
        if traded == best && traded > 0 {
            optimal.push(price);
        }
    }
    let (lowest, highest) = (*optimal.first()?, *optimal.last()?);
    let middle = lowest + (highest - lowest) / 2;
    let price = if volume(middle) == best {
        middle
    } else {
        optimal.iter().copied().min_by_key(|candidate| ((candidate - middle).abs(), *candidate))?
    };
    Some((Clearing { price, volume: best, lowest, highest }, match_at(bids, asks, may_trade, price)))
}

/// `clear_pairs` without restrictions on (price, units) pairs, every
/// order its own account: the plain auction math.
pub fn clearing(bids: &[(i64, i64)], asks: &[(i64, i64)]) -> Option<Clearing> {
    let orders = |side: &str, rows: &[(i64, i64)]| -> Vec<AuctionOrder> { rows.iter().enumerate().map(|(index, (price, quantity))| AuctionOrder { account: format!("{side}{index}"), price: *price, quantity: *quantity }).collect() };
    clear_pairs(&orders("b", bids), &orders("s", asks), &|_, _| true).map(|(found, _)| found)
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

/// Auctions kept to measure an auction object's equivalent gap.
pub const AUCTION_HISTORY: usize = 4;
/// Auctions needed before the equivalent gap speaks (one burst is no trend).
pub const MIN_AUCTIONS_FOR_GAP: usize = 2;

/// Equivalent time between two trades of an object in auction mode
/// (review lot 3, B3): in that mode it trades once per auction, so
/// `trade_gap_ticks` on its executions can never fall under a period
/// and the object would never come back. The same quantity, measured
/// independently of the mode: `AUCTION_PERIOD_TICKS` over the mean
/// number of PAIRS served per auction (one pair = one deal = one
/// execution, the unit the continuous gap counts), over the last
/// `AUCTION_HISTORY` auctions. `None` before `MIN_AUCTIONS_FOR_GAP`
/// auctions (the executions then decide); infinite when none served.
pub fn equivalent_gap_ticks(pairs_per_auction: &[u32]) -> Option<f64> {
    if pairs_per_auction.len() < MIN_AUCTIONS_FOR_GAP {
        return None;
    }
    let recent = &pairs_per_auction[pairs_per_auction.len().saturating_sub(AUCTION_HISTORY)..];
    let mean = recent.iter().map(|pairs| f64::from(*pairs)).sum::<f64>() / recent.len() as f64;
    Some(if mean <= 0.0 { f64::INFINITY } else { AUCTION_PERIOD_TICKS as f64 / mean })
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
