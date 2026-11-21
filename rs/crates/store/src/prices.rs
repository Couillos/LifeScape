#![forbid(unsafe_code)]
//! Cheap price reads for the bots (user 2026-11-14: "attention au cpu").
//! The current price is the median of the last five fills clamped by
//! the impact prices of the book (`lifescape_market::current_price`):
//! one indexed `LIMIT` read on `idx_executions_item_clock`, never the
//! whole history of an item, and one indexed read of the best open
//! orders of a side, stopped once the impact size is covered. The 24 h
//! volume is one indexed `SUM`, never the rows.

use crate::{Error, OrderSide, Store};

/// Open orders an impact walk reads at most per side: a side whose
/// first `IMPACT_ROWS` orders hold fewer units than the impact size is
/// read as thin (the live engine book walks every order; a side of more
/// than 64 dust orders is the only case where both differ).
pub const IMPACT_ROWS: i64 = 64;

impl Store {
    /// (price, quantity) of the newest `limit` fills of one item, oldest
    /// first (any age).
    /// Store clock of the newest fill of one item (`None`: never traded):
    /// how old the current price's prints are (plan prix §3).
    pub fn newest_fill_clock(&self, item_id: i64) -> Result<Option<i64>, Error> {
        let mut statement = self.connection.prepare_cached("SELECT MAX(clock) FROM executions WHERE item_id = ?1")?;
        Ok(statement.query_row(rusqlite::params![item_id], |row| row.get::<_, Option<i64>>(0))?)
    }

    pub fn last_fills(&self, item_id: i64, limit: i64) -> Result<Vec<(i64, i64)>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT price, quantity FROM executions WHERE item_id = ?1 \
             ORDER BY clock DESC, exec_id DESC LIMIT ?2",
        )?;
        let mut rows = statement.query(rusqlite::params![item_id, limit.max(0)])?;
        let mut fills = Vec::new();
        while let Some(row) = rows.next()? {
            fills.push((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?));
        }
        fills.reverse();
        Ok(fills)
    }

    /// Clocks of the item's newest `limit` fills, newest first (B3: the
    /// mean time between two trades; one indexed `LIMIT` read).
    pub fn fill_clocks(&self, item_id: i64, limit: i64) -> Result<Vec<i64>, Error> {
        let mut statement = self.connection.prepare_cached("SELECT clock FROM executions WHERE item_id = ?1 ORDER BY clock DESC, exec_id DESC LIMIT ?2")?;
        let mut rows = statement.query(rusqlite::params![item_id, limit.max(0)])?;
        let mut clocks = Vec::new();
        while let Some(row) = rows.next()? {
            clocks.push(row.get::<_, i64>(0)?);
        }
        Ok(clocks)
    }

    /// The item's newest `limit` settled deals since `since_clock`
    /// (store clock), newest first, WITHOUT those of `excluded_account`
    /// (B4: the anchor leaves out the asking account's own fills; the
    /// exclusion is in the query, so the limit counts only the others;
    /// executions carry no account). Indexed on `schema::DEAL_ITEM_INDEX`.
    pub fn settled_fills_since(&self, item_id: i64, since_clock: i64, excluded_account: &str, limit: i64) -> Result<Vec<SettledFill>, Error> {
        let mut statement = self.connection.prepare_cached(
            "SELECT price, quantity, buyer, seller FROM deals \
             WHERE item_id = ?1 AND status = 'settled' AND updated_clock >= ?2 AND buyer <> ?3 AND seller <> ?3 \
             ORDER BY updated_clock DESC LIMIT ?4",
        )?;
        let mut rows = statement.query(rusqlite::params![item_id, since_clock, excluded_account, limit.max(0)])?;
        let mut fills = Vec::new();
        while let Some(row) = rows.next()? {
            fills.push(SettledFill { price: row.get(0)?, quantity: row.get(1)?, buyer: row.get(2)?, seller: row.get(3)? });
        }
        Ok(fills)
    }

    /// Open (non-dormant) levels of one side of an item, best first,
    /// (price, remaining), until `units` are covered or `IMPACT_ROWS`
    /// orders were read (indexed on `idx_orders_item_status`, `LIMIT`).
    pub fn book_levels_covering(&self, item_id: i64, side: OrderSide, units: i64) -> Result<Vec<(i64, i64)>, Error> {
        let mut statement = self.connection.prepare_cached(match side {
            OrderSide::Buy => {
                "SELECT price, remaining FROM orders \
                 WHERE item_id = ?1 AND status = 'open' AND side = 'buy' AND dormant = 0 AND remaining > 0 \
                 ORDER BY price DESC, created_clock ASC LIMIT ?2"
            }
            OrderSide::Sell => {
                "SELECT price, remaining FROM orders \
                 WHERE item_id = ?1 AND status = 'open' AND side = 'sell' AND dormant = 0 AND remaining > 0 \
                 ORDER BY price ASC, created_clock ASC LIMIT ?2"
            }
        })?;
        let mut rows = statement.query(rusqlite::params![item_id, IMPACT_ROWS])?;
        let mut levels: Vec<(i64, i64)> = Vec::new();
        let mut covered = 0;
        while let Some(row) = rows.next()? {
            let (price, remaining) = (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?);
            match levels.last_mut() {
                Some(last) if last.0 == price => last.1 += remaining,
                _ => levels.push((price, remaining)),
            }
            covered += remaining;
            if covered >= units.max(1) {
                break;
            }
        }
        Ok(levels)
    }

    /// Units traded per item since `from_clock` (items with at least one
    /// fill), item id order.
    pub fn volumes_since(&self, from_clock: i64) -> Result<Vec<(i64, i64)>, Error> {
        let mut statement = self.connection.prepare_cached("SELECT item_id, SUM(quantity) FROM executions WHERE clock >= ?1 GROUP BY item_id ORDER BY item_id")?;
        let rows = statement.query_map(rusqlite::params![from_clock], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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

/// One settled deal as the anchor reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct SettledFill {
    pub price: i64,
    pub quantity: i64,
    pub buyer: String,
    pub seller: String,
}

#[cfg(test)]
mod tests {
    use crate::{NewExecution, Store};

    fn fill(clock: i64, price: i64, quantity: i64) -> NewExecution {
        NewExecution { item_id: 314, quantity, price, clock, server_tick: clock, taker_side: None }
    }

    #[test]
    fn volumes_since_sums_each_item_after_the_clock() -> Result<(), crate::Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&crate::Item { item_id: 314, name: "Feather".to_string(), shop_price: 0, alch_value: 0 })?;
        for (clock, quantity) in [(10, 5), (50, 2), (60, 3)] {
            store.record_execution(&fill(clock, 1, quantity))?;
        }
        assert_eq!(store.volumes_since(40)?, vec![(314, 5)]);
        assert_eq!(store.volumes_since(0)?, vec![(314, 10)]);
        Ok(())
    }

    #[test]
    fn last_fills_are_the_newest_fills_oldest_first() -> Result<(), crate::Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&crate::Item { item_id: 314, name: "Feather".to_string(), shop_price: 0, alch_value: 0 })?;
        for (clock, price) in [(10, 1), (20, 2), (30, 3), (40, 4), (50, 5), (60, 6), (70, 7)] {
            store.record_execution(&fill(clock, price, 2))?;
        }
        assert_eq!(store.last_fills(314, 5)?, vec![(3, 2), (4, 2), (5, 2), (6, 2), (7, 2)]);
        assert_eq!(store.last_fills(999, 5)?, Vec::<(i64, i64)>::new());
        assert_eq!(store.newest_fill_clock(999)?, None, "never traded");
        assert!(store.newest_fill_clock(314)?.is_some());
        assert_eq!(store.volume_between(314, 30, 61)?, 8);
        assert_eq!(store.volume_between(314, 100, 200)?, 0);
        Ok(())
    }

    #[test]
    fn book_levels_stop_once_the_impact_size_is_covered() -> Result<(), crate::Error> {
        let store = Store::open_in_memory()?;
        store.upsert_item(&crate::Item { item_id: 314, name: "Feather".to_string(), shop_price: 0, alch_value: 0 })?;
        let order = |side, quantity, price, dormant| crate::NewOrder { account: "t01".to_string(), side, item_id: 314, quantity, price, dormant, clock: 1, ..crate::NewOrder::default() };
        store.place_order(&order(crate::OrderSide::Sell, 1, 1, false))?;
        store.place_order(&order(crate::OrderSide::Sell, 60, 4, false))?;
        store.place_order(&order(crate::OrderSide::Sell, 30, 4, false))?;
        store.place_order(&order(crate::OrderSide::Sell, 500, 6, false))?;
        store.place_order(&order(crate::OrderSide::Sell, 500, 9, false))?;
        store.place_order(&order(crate::OrderSide::Sell, 900, 2, true))?;
        store.place_order(&order(crate::OrderSide::Buy, 10, 3, false))?;
        assert_eq!(store.book_levels_covering(314, crate::OrderSide::Sell, 100)?, vec![(1, 1), (4, 90), (6, 500)], "dormant orders never rest");
        assert_eq!(store.book_levels_covering(314, crate::OrderSide::Sell, 1)?, vec![(1, 1)]);
        assert_eq!(store.book_levels_covering(314, crate::OrderSide::Buy, 100)?, vec![(3, 10)], "thin side: all it has");
        Ok(())
    }
}
