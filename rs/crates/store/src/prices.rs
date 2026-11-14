#![forbid(unsafe_code)]
//! Cheap price reads for the bots (user 2026-11-14: "attention au cpu").
//! The current price is the median of the last five fills
//! (`lifescape_market::current_price`): one indexed `LIMIT` read on
//! `idx_executions_item_clock`, never the whole history of an item. The
//! 24 h volume is one indexed `SUM`, never the rows.

use crate::{Error, Store};

impl Store {
    /// Prices of the newest `limit` fills of one item, oldest first (any
    /// age).
    pub fn last_prices(&self, item_id: i64, limit: i64) -> Result<Vec<i64>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT price FROM executions WHERE item_id = ?1 \
             ORDER BY clock DESC, exec_id DESC LIMIT ?2",
        )?;
        let mut rows = statement.query(rusqlite::params![item_id, limit.max(0)])?;
        let mut prices = Vec::new();
        while let Some(row) = rows.next()? {
            prices.push(row.get::<_, i64>(0)?);
        }
        prices.reverse();
        Ok(prices)
    }

    /// Units of one item traded in `[from_clock, to_clock)`.
    pub fn volume_between(&self, item_id: i64, from_clock: i64, to_clock: i64) -> Result<i64, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT COALESCE(SUM(quantity), 0) FROM executions \
             WHERE item_id = ?1 AND clock >= ?2 AND clock < ?3",
        )?;
        Ok(statement.query_row(rusqlite::params![item_id, from_clock, to_clock], |row| row.get::<_, i64>(0))?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{NewExecution, Store};

    fn fill(clock: i64, price: i64, quantity: i64) -> NewExecution {
        NewExecution { item_id: 314, quantity, price, clock, server_tick: clock, taker_side: None }
    }

    #[test]
    fn last_prices_are_the_newest_fills_oldest_first() -> Result<(), crate::Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&crate::Item { item_id: 314, name: "Feather".to_string(), shop_price: 0, alch_value: 0 })?;
        for (clock, price) in [(10, 1), (20, 2), (30, 3), (40, 4), (50, 5), (60, 6), (70, 7)] {
            store.record_execution(&fill(clock, price, 2))?;
        }
        assert_eq!(store.last_prices(314, 5)?, vec![3, 4, 5, 6, 7]);
        assert_eq!(store.last_prices(999, 5)?, Vec::<i64>::new());
        assert_eq!(store.volume_between(314, 30, 61)?, 8);
        assert_eq!(store.volume_between(314, 100, 200)?, 0);
        Ok(())
    }
}
