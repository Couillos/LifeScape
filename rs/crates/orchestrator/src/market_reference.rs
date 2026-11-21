#![forbid(unsafe_code)]
//! Prices of the daemon market (split from `market_service`, rule 4):
//! the last fills of an object, its current price (`reference`, per
//! unit) and the same in the book's own unit (`reference_lot`, coins per
//! lot, the repricing and the market's internal walks), and the two ops
//! that read them (`quote`, `book`).

use lifescape_json::{object, str_value, Value};
use lifescape_market::{impact_size, LastFill, Side, PRICE_FILLS};

use crate::market_service::MarketService;

impl MarketService {
    /// Last fills (price, quantity) of one item, oldest first (at most
    /// `PRICE_FILLS`, any age): the in-memory ring, loaded once from one
    /// indexed `LIMIT` read (never a scan of the item's history).
    pub(crate) fn last_fills(&self, item: i64) -> Vec<LastFill> {
        let seen = {
            let engine = self.lock_engine();
            if let Some(ring) = engine.last_fills.get(&item) {
                return ring.iter().copied().collect();
            }
            engine.fills_settled
        };
        let loaded: Vec<LastFill> = self
            .lock_store()
            .last_fills(item, PRICE_FILLS as i64)
            .unwrap_or_default()
            .into_iter()
            .map(|(price, quantity)| LastFill { price, quantity })
            .collect();
        let mut engine = self.lock_engine();
        if engine.fills_settled == seen {
            engine.last_fills.insert(item, loaded.iter().copied().collect());
        }
        loaded
    }

    /// Last fill prices of one item, oldest first (`last_fills`), per
    /// unit (plan prix lot D: the brain's unit).
    pub(crate) fn last_fill_prices(&self, item: i64) -> Vec<i64> {
        self.last_fills(item).iter().map(|fill| crate::market_units::to_unit(item, fill.price)).collect()
    }

    /// Current price of one item (user 2026-11-14, replaces the 24 h
    /// VWAP in every bot decision): median of the last five fills
    /// clamped by the impact prices of the live book (the price of the
    /// median lot's last unit on each side; a thinner side never
    /// clamps), else the book, else 1 coin
    /// (`lifescape_market::current_price`). One walk over the few best
    /// orders of each side. The shop price never anchors it.
    pub(crate) fn reference(&self, item: i64) -> lifescape_market::Reference {
        let reference = self.reference_lot(item);
        lifescape_market::Reference { price: crate::market_units::to_unit(item, reference.price), ..reference }
    }

    /// `reference` in the book's own unit (plan prix lot D: coins per lot):
    /// the repricing and the market's internal walks.
    pub(crate) fn reference_lot(&self, item: i64) -> lifescape_market::Reference {
        let last = self.last_fills(item);
        let newest = self.newest_fill_clock(item);
        let size = impact_size(&last);
        let (bids, asks) = self
            .lock_engine()
            .books
            .get(&item)
            .map(|book| (book.levels_covering(Side::Buy, size), book.levels_covering(Side::Sell, size)))
            .unwrap_or_default();
        lifescape_market::current_price_at(&last, &bids, &asks, newest, Self::now_clock())
    }

    /// Store clock of the item's newest fill (plan prix §3), cached in the
    /// engine and moved by every settled fill (`note_fill`).
    pub(crate) fn newest_fill_clock(&self, item: i64) -> Option<i64> {
        if let Some(clock) = self.lock_engine().newest_fill.get(&item).copied() {
            return Some(clock);
        }
        let clock = self.lock_store().newest_fill_clock(item).ok().flatten()?;
        self.lock_engine().newest_fill.insert(item, clock);
        Some(clock)
    }

    pub(crate) fn op_quote(&self, request: &Value) -> Value {
        // One book per object: a banknote asks about its object.
        let item = request.get("item").as_i64().map(crate::market_audit::book_item).unwrap_or(-1);
        let engine = self.lock_engine();
        let Some(row) = engine.find_item(item) else {
            return Self::err(format!("unknown item {item}"));
        };
        let (bid, ask) = engine.books.get(&item).map(|book| book.best()).unwrap_or((None, None));
        // Per unit: the scripts price their orders per unit (plan prix lot D).
        let (bid, ask) = (bid.map(|price| crate::market_units::to_unit(item, price)), ask.map(|price| crate::market_units::to_unit(item, price)));
        let name = row.name.clone();
        let shop = row.shop_price;
        drop(engine);
        let reference = self.reference(item);
        // Last print: the price ring (loaded from the store once, so it
        // survives a daemon restart).
        let last = self.last_fill_prices(item).last().copied();
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("name", str_value(&name)),
            ("bid", bid.map(Value::Int).unwrap_or(Value::Null)),
            ("ask", ask.map(Value::Int).unwrap_or(Value::Null)),
            ("reference", Value::Int(reference.price)),
            ("traded", Value::Bool(reference.traded)),
            ("shop", Value::Int(shop)),
            ("last", last.map(Value::Int).unwrap_or(Value::Null)),
        ])
    }

    pub(crate) fn op_book(&self, request: &Value) -> Value {
        // One book per object: a banknote asks about its object.
        let item = request.get("item").as_i64().map(crate::market_audit::book_item).unwrap_or(-1);
        let depth = request.get("depth").as_i64().unwrap_or(10).clamp(1, 50) as usize;
        let engine = self.lock_engine();
        if engine.find_item(item).is_none() {
            return Self::err(format!("unknown item {item}"));
        }
        let (buys, sells) = match engine.books.get(&item) {
            Some(book) => (book.depth(Side::Buy, depth), book.depth(Side::Sell, depth)),
            None => (Vec::new(), Vec::new()),
        };
        let rows = |levels: Vec<(i64, i64)>| {
            Value::Arr(levels.into_iter().map(|(price, qty)| Value::Arr(vec![Value::Int(price), Value::Int(qty)])).collect())
        };
        // Coins per `lot` units (plan prix lot D): the page labels them.
        object(vec![
            ("ok", Value::Bool(true)),
            ("item", Value::Int(item)),
            ("lot", Value::Int(crate::market_units::lot_of(item))),
            ("buys", rows(buys)),
            ("sells", rows(sells)),
        ])
    }
}
