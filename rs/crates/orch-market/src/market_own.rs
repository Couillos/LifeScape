#![forbid(unsafe_code)]
//! The market engine's memory of a seller's own trades and the public
//! book as a seller reads it (`BookView`): the part of the seller side
//! that only needs the engine (own fills and their speed, last own
//! purchase, unsold horizons, overstock). The pricing policy that sits on
//! top (`sale_decision`, the `seller_quote` op) is the brain's
//! (`market_seller`). Split out of `market_seller` in step B4
//! (2026-11-23): an inherent impl cannot live in another crate.

use lifescape_market::OwnFill;
use lifescape_planner::market_fit::BookView;

use crate::market_service::MarketService;

/// Plan prix §1: how far back a seller's unsold horizons and sales count
/// (one day: an old failure fades).
pub const LEARNING_WINDOW_SECS: i64 = 86_400;

impl MarketService {
    /// The public book of an item as a seller reads it (`BookView`):
    /// every OTHER account's bids, every resting ask, 24 h volume.
    pub fn public_view(&self, account: &str, item: i64) -> BookView {
        let clock = Self::now_clock();
        // One indexed SUM, never the rows (CPU: hottest pool call).
        let volume_24h: i64 = self.read_store().volume_between(item, clock - 86_400, clock + 1).unwrap_or(0);
        let mut view = BookView { volume_24h, ..BookView::default() };
        if let Some(book) = self.lock_engine().books.get(&item) {
            // Per unit (plan prix lot D: the brain's unit).
            for order in book.orders().into_iter().filter(|order| order.remaining > 0) {
                let price = crate::market_units::to_unit(item, order.price);
                if order.side == lifescape_market::Side::Sell {
                    view.asks.push((price, order.remaining));
                } else if order.account != account {
                    view.bids.push((price, order.remaining));
                }
            }
        }
        view
    }

    /// Records the seller's own fill with how long its listing rested
    /// (from `QuoteBounds::listed_wall`; a sell that crossed a bid on
    /// arrival rested 0). Called by every match (`record_fill`).
    pub fn remember_fill(&self, fill: &lifescape_market::Fill) {
        let tick_ms = crate::market_eta::live_tick_ms().max(1) as f64;
        let mut engine = self.lock_engine();
        let listed = engine.quote_bounds.get(&(fill.sell_id as i64)).map(|bounds| bounds.listed_wall);
        let rested_ticks = match listed {
            Some(wall) => Some(((crate::market_convene::wall_now() - wall).max(0.0) * 1000.0 / tick_ms) as u64),
            None if fill.taker == lifescape_market::Side::Sell => Some(0),
            None => None,
        };
        engine.own_sells.insert((fill.seller.clone(), fill.item), OwnFill { price: fill.price, rested_ticks });
    }

    /// The account's own last sale of `item`: memory (with its speed),
    /// else its newest non-failed seller deal (speed unknown).
    pub fn own_sell_fill(&self, account: &str, item: i64) -> Option<OwnFill> {
        // Per unit (plan prix lot D): the memory and the carnet hold the
        // book's price per lot.
        let unit = |fill: OwnFill| OwnFill { price: crate::market_units::to_unit(item, fill.price), ..fill };
        if let Some(fill) = self.lock_engine().own_sells.get(&(account.to_string(), item)).copied() {
            return Some(unit(fill));
        }
        let stored = self.read_store().last_own_fill(account, item, false).ok().flatten();
        stored.map(|(price, _)| unit(OwnFill { price, rested_ticks: None }))
    }

    /// The account's own last purchase price of `item` (non-failed deal),
    /// per unit.
    pub fn own_buy_price(&self, account: &str, item: i64) -> Option<i64> {
        self.own_buy_price_lot(account, item).map(|price| crate::market_units::to_unit(item, price))
    }

    /// `own_buy_price` in the book's unit (per lot).
    pub fn own_buy_price_lot(&self, account: &str, item: i64) -> Option<i64> {
        self.read_store().last_own_fill(account, item, true).ok().flatten().map(|(price, _)| price)
    }

    /// Plan prix §1: the account's unsold horizons per object over the last
    /// day, net of its sales (one carnet read per selling pass).
    pub fn seller_learning(&self, account: &str) -> std::collections::HashMap<i64, i64> {
        let since = Self::now_clock() - LEARNING_WINDOW_SECS;
        let rows = self.read_store().seller_learning(account, since).unwrap_or_default();
        rows.into_iter().map(|(item, (withdrawn, sold))| (item, lifescape_market::discovery::unsold_horizons(withdrawn, sold))).collect()
    }

    /// One object's unsold horizons (`seller_learning`).
    pub fn unsold_horizons(&self, account: &str, item: i64) -> i64 {
        self.seller_learning(account).get(&item).copied().unwrap_or(0)
    }

    /// True when `stock` free units of `item` pass what the market
    /// absorbs (`market_fit::overstocked`): the lot goes to the market,
    /// never diverted to a shop that takes a few units.
    pub fn overstocked(&self, account: &str, item: i64, stock: i64) -> bool {
        lifescape_planner::market_fit::overstocked(stock, &self.public_view(account, item))
    }
}
