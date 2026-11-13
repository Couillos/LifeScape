#![forbid(unsafe_code)]
//! The book is never crossed (live 2026-11-13, bronze axe 1351: a deal
//! that failed relisted lara_lane's bid at 20 above asks at 16 without
//! matching; the book stayed crossed and a later seller hit the bid at
//! 20 while the 16 asks rested).
//!
//! - A relist after a failed deal re-enters as a TAKER
//!   (`Book::relist_matching`): it fills at the resting orders' prices
//!   (a buyer pays the best ask, a seller gets the best bid) and only its
//!   remainder rests, at its original rank.
//! - The two accounts of a failed deal never match EACH OTHER for
//!   `PAIR_COOLDOWN_TICKS` (they would meet and fail again at once);
//!   they match anybody else. Every failure reason arms it: a fault
//!   verdict relists one side only, so the guard only matters for that
//!   side's other orders, which must not rematch the partner either.
//! - `uncross_books` (boot reload, then every convene round) matches
//!   whatever still crosses: a carnet left crossed by the old relist,
//!   pairs whose cooldown just expired. The later (re)entry is the taker.
//!
//! Every fill goes through the normal path, exactly as a `place` taker:
//! `merge_fills`, `record_fill` (deal, ledger, relist rows), then
//! `refresh_order_rows` (carnet statuses). Lock order: the book moves in
//! one engine section, released before any store call; `record_fill`
//! and `refresh_order_rows` then alternate engine and store sections,
//! never nested (same rule as `market_orders`).

use std::collections::HashMap;

use lifescape_market::{Fill, Order};

use crate::market_service::{Engine, MarketService};

/// Cooldown between the two accounts of a failed deal, in server ticks
/// (3000 ticks = 30 min at 600 ms, 3 min on the 60 ms dev server):
/// long enough for both bots to move on (new prices, other partners),
/// short enough that two lone traders of one good meet again.
pub(crate) const PAIR_COOLDOWN_TICKS: u64 = 3000;

/// Cooldown length in wall seconds at the live tick rate (the configured
/// rate before the convene loop measured one, e.g. at boot).
pub(crate) fn cooldown_secs() -> f64 {
    PAIR_COOLDOWN_TICKS as f64 * crate::market_eta::live_tick_ms() as f64 / 1000.0
}

fn pair_key(first: &str, second: &str) -> (String, String) {
    if first <= second {
        (first.to_string(), second.to_string())
    } else {
        (second.to_string(), first.to_string())
    }
}

/// Pair guard for the book at `now_wall`: true while the cooldown of
/// (buyer, seller) runs.
pub(crate) fn kept_apart(cooldowns: &HashMap<(String, String), f64>, now_wall: f64) -> impl Fn(&str, &str) -> bool + '_ {
    move |buyer: &str, seller: &str| cooldowns.get(&pair_key(buyer, seller)).is_some_and(|until| *until > now_wall)
}

/// Consumed rows of a fill between two orders known before the match
/// (resting rows, or the relisted order itself): each keeps its `seq`
/// so a failed handover relists both at their rank. Ids are
/// placeholders (0), assigned at relist time.
fn rows_for_fill(pre: &HashMap<u64, Order>, fill: &Fill) -> Vec<Order> {
    [fill.buy_id, fill.sell_id]
        .iter()
        .filter_map(|id| pre.get(id))
        .map(|order| Order { id: 0, quantity: fill.quantity, remaining: fill.quantity, ..order.clone() })
        .collect()
}

fn snapshot(book: &lifescape_market::Book) -> HashMap<u64, Order> {
    book.orders().into_iter().map(|order| (order.id, order)).collect()
}

impl MarketService {
    /// Keeps `buyer` and `seller` from matching each other until
    /// `until_wall` (daemon wall seconds); never shortens a running one.
    pub(crate) fn hold_pair_apart(&self, buyer: &str, seller: &str, until_wall: f64) {
        let mut engine = self.lock_engine();
        let until = engine.pair_cooldowns.entry(pair_key(buyer, seller)).or_insert(until_wall);
        *until = until.max(until_wall);
    }

    /// Re-lists one order (carnet row already open under `order.id`) as
    /// a taker. Returns the deals its fills opened.
    pub(crate) fn relist_order(&self, order: Order) -> Vec<i64> {
        let order_id = order.id;
        let item = order.item;
        let matched = {
            let mut guard = self.lock_engine();
            let engine: &mut Engine = &mut guard;
            let guard_fn = kept_apart(&engine.pair_cooldowns, crate::market_convene::wall_now());
            let book = engine.books.entry(item).or_default();
            let mut pre = snapshot(book);
            pre.insert(order_id, order.clone());
            let fills = book.relist_matching(order, &guard_fn);
            engine.order_item.insert(order_id, item);
            fills.into_iter().map(|fill| (fill.clone(), rows_for_fill(&pre, &fill))).collect::<Vec<_>>()
        };
        self.settle_crossed(order_id, matched)
    }

    /// Matches every crossed pair left in the books once the expired
    /// cooldowns are dropped. Returns the deals opened.
    pub(crate) fn uncross_books(&self, now_wall: f64) -> Vec<i64> {
        let matched = {
            let mut guard = self.lock_engine();
            let engine: &mut Engine = &mut guard;
            engine.pair_cooldowns.retain(|_, until| *until > now_wall);
            let guard_fn = kept_apart(&engine.pair_cooldowns, now_wall);
            let mut items: Vec<i64> = engine.books.keys().copied().collect();
            items.sort_unstable();
            let mut matched: Vec<(Fill, Vec<Order>)> = Vec::new();
            for item in items {
                let Some(book) = engine.books.get_mut(&item) else {
                    continue;
                };
                let pre = snapshot(book);
                for fill in book.uncross(&guard_fn) {
                    let rows = rows_for_fill(&pre, &fill);
                    matched.push((fill, rows));
                }
            }
            matched
        };
        let Some(first) = matched.first().map(|(fill, _)| fill.buy_id) else {
            return Vec::new();
        };
        crate::logln!("market-uncross: {} crossed fill(s) matched", matched.len());
        self.settle_crossed(first, matched)
    }

    /// Restores the cooldowns of the deals that failed within the window
    /// (boot: the table died with the previous daemon), each counted from
    /// its failure instant.
    pub(crate) fn restore_pair_cooldowns(&self) {
        let window = cooldown_secs();
        let now = Self::now_clock();
        let since = now - window.ceil() as i64;
        let failed = match self.lock_store().failed_deals_since(since) {
            Ok(failed) => failed,
            Err(error) => {
                crate::logln!("market reload: pair cooldowns not restored, store refused: {error}");
                return;
            }
        };
        for deal in &failed {
            self.hold_pair_apart(&deal.buyer, &deal.seller, deal.clock as f64 + window);
        }
    }

    /// The normal fill path for fills made outside `place`: one deal per
    /// counterparty and price, then the carnet rows of every order touched.
    fn settle_crossed(&self, incoming: u64, matched: Vec<(Fill, Vec<Order>)>) -> Vec<i64> {
        if matched.is_empty() {
            return Vec::new();
        }
        let clock = Self::now_clock();
        let fills: Vec<Fill> = matched.iter().map(|(fill, _)| fill.clone()).collect();
        let mut deals = Vec::new();
        for (fill, rows) in crate::market_merge::merge_fills(matched) {
            match self.record_fill(&fill, rows, clock) {
                Ok(deal_id) => deals.push(deal_id),
                Err(error) => crate::logln!(
                    "market-uncross: {} x {} obj {} at {}: deal not recorded ({error})",
                    fill.buyer,
                    fill.seller,
                    fill.item,
                    fill.price
                ),
            }
        }
        self.refresh_order_rows(incoming, &fills, clock);
        deals
    }
}
