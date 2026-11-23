//! Amend in place (schema 12, user 2026-11-22: « 80 % des transactions
//! annulées, ça doit être à 0 % »). Before, a reprice cancelled the row
//! and inserted a replacement (`replaces`): ten walks of one order were
//! ten « cancellations ». Now the same row changes price, remaining,
//! ceiling and ask limit; the walk's history lives in `order_amends`
//! (read by the market criteria: opening price, number of amends).

use rusqlite::params;

use crate::{Error, Store};

/// Migration 12: the amend journal.
pub(crate) const MIGRATION_V12: &str = "\
CREATE TABLE IF NOT EXISTS order_amends (
    amend_id INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id INTEGER NOT NULL,
    clock INTEGER NOT NULL,
    old_price INTEGER NOT NULL,
    new_price INTEGER NOT NULL,
    old_remaining INTEGER NOT NULL,
    new_remaining INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_order_amends_order ON order_amends(order_id);
CREATE INDEX IF NOT EXISTS idx_order_amends_clock ON order_amends(clock);
PRAGMA user_version = 12;";

/// What an in-place amend changes.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderAmend {
    pub order_id: i64,
    pub price: i64,
    /// New remaining (never above the current one); `None` keeps it.
    pub remaining: Option<i64>,
    /// New ceiling / ask limit; `None` keeps the stored one.
    pub ceiling: Option<i64>,
    pub ask_limit: Option<i64>,
    pub clock: i64,
}

impl Store {
    /// Amends a resting order (open or partly matched) in place and
    /// journals the change.
    pub fn amend_order_in_place(&self, amend: &OrderAmend) -> Result<(), Error> {
        if amend.price < 1 {
            return Err(Error::Invalid(format!("bad amended price {}", amend.price)));
        }
        let current = self
            .get_order(amend.order_id)?
            .filter(|row| matches!(row.status, crate::OrderStatus::Open | crate::OrderStatus::Matched))
            .ok_or_else(|| Error::NotFound(format!("open order {}", amend.order_id)))?;
        let remaining = amend.remaining.map(|cut| cut.min(current.remaining)).unwrap_or(current.remaining);
        if remaining < 1 {
            return Err(Error::Invalid(format!("bad amended remaining {remaining}")));
        }
        let quantity = current.quantity - (current.remaining - remaining);
        let touched = self.run(
            "UPDATE orders SET price = ?1, remaining = ?2, quantity = ?3, updated_clock = ?4, \
             ceiling = COALESCE(?5, ceiling), ask_limit = COALESCE(?6, ask_limit) \
             WHERE order_id = ?7 AND status IN ('open','matched')",
            params![amend.price, remaining, quantity, amend.clock, amend.ceiling, amend.ask_limit, amend.order_id],
        )?;
        if touched == 0 {
            return Err(Error::NotFound(format!("open order {}", amend.order_id)));
        }
        self.run(
            "INSERT INTO order_amends(order_id, clock, old_price, new_price, old_remaining, new_remaining) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![amend.order_id, amend.clock, current.price, amend.price, current.remaining, remaining],
        )?;
        Ok(())
    }

    /// Price an order opened at: the first amend's old price, else the
    /// row's own price.
    pub fn order_open_price(&self, order_id: i64) -> Result<Option<i64>, Error> {
        let first = self.connection.query_row("SELECT old_price FROM order_amends WHERE order_id = ?1 ORDER BY amend_id LIMIT 1", params![order_id], |row| row.get::<_, i64>(0));
        match first {
            Ok(price) => Ok(Some(price)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(self.get_order(order_id)?.map(|row| row.price)),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewOrder, OrderSide, OrderStatus};

    fn sample(account: &str) -> NewOrder {
        NewOrder { account: account.to_string(), side: OrderSide::Buy, item_id: 314, quantity: 100, price: 3, dormant: false, clock: 1_700_000_000, replaces: None, motive: "need".to_string(), ceiling: None, ask_limit: None }
    }

    /// Ten walks of one order: one row, still open, never cancelled; the
    /// journal keeps the history.
    #[test]
    fn an_amend_rewrites_the_row_and_cancels_nothing() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let order = store.place_order(&sample("t01"))?;
        for step in 1..=10 {
            store.amend_order_in_place(&OrderAmend { order_id: order, price: 3 + step, remaining: Some(100 - step), ceiling: Some(50), ask_limit: None, clock: 1_700_000_000 + step * 10 })?;
        }
        let row = store.get_order(order)?.expect("row");
        assert_eq!((row.status, row.price, row.remaining, row.quantity, row.ceiling, row.cancel_reason.as_str()), (OrderStatus::Open, 13, 90, 90, Some(50), ""));
        assert_eq!((row.root_id, row.root_clock, row.created_clock), (order, 1_700_000_000, 1_700_000_000), "t0 never moves");
        assert_eq!(store.order_open_price(order)?, Some(3));
        assert_eq!(store.account_orders("t01")?.len(), 1);
        let journal: i64 = store.connection.query_row("SELECT COUNT(*) FROM order_amends WHERE order_id = ?1", params![order], |row| row.get(0))?;
        assert_eq!(journal, 10);
        Ok(())
    }

    #[test]
    fn a_closed_order_is_never_amended() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let order = store.place_order(&sample("t02"))?;
        store.cancel_order(order, 1_700_000_050)?;
        let refused = store.amend_order_in_place(&OrderAmend { order_id: order, price: 9, remaining: None, ceiling: None, ask_limit: None, clock: 1_700_000_100 });
        assert!(matches!(refused, Err(Error::NotFound(_))), "{refused:?}");
        Ok(())
    }
}
