#![forbid(unsafe_code)]
//! Retention of cancelled orders (plan d'architecture, decision 3,
//! 2026-11-24): the carnet kept every cancelled order forever (1.3 million
//! of the 1.46 million rows, 108 MB), nobody reads them past a few days.
//!
//! What is purged: a `cancelled` order last updated before the cutoff,
//! with its `order_amends` journal, once the whole lineage it belongs to
//! (`root_id`) is over too: no sibling that is still open / matched /
//! settled, none updated after the cutoff. A buy's lineage is measured as
//! a whole by `market_criteria` (its start, its price walk, its end), so a
//! live chain is never cut in the middle. Open, matched and settled
//! orders, deals and executions are never touched.
//!
//! Readers of cancelled rows and their windows (checked 2026-11-24): the
//! seller's learned floor (`seller_learning`, `cancel_reason = 'horizon'`)
//! reads the last 24 h; the economy page's order outcomes read 1 h, 24 h,
//! 7 days and "all"; `market_criteria` reads the lineages touched in the
//! window of `economy [hours]`. The cutoff (14 days) exceeds the 24 h and
//! the 7 days; only the page's "all" window and an `economy` window longer
//! than the retention see less than before.

use rusqlite::params;

use crate::{Error, Store};

/// What a purge counted or removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExpiredOrders {
    /// Cancelled orders past the retention.
    pub orders: i64,
    /// Their `order_amends` journal rows.
    pub amends: i64,
}

/// The `WHERE` of an expired order, for the table aliased `table`: cancelled,
/// last updated before `?1`, and its lineage finished (no sibling open,
/// matched or settled, none updated since `?1`). A row without lineage
/// (`root_id` 0) stands alone.
fn expired(table: &str) -> String {
    format!(
        "{table}.status = 'cancelled' AND {table}.updated_clock < ?1 AND ({table}.root_id = 0 OR NOT EXISTS (\
         SELECT 1 FROM orders sibling WHERE sibling.root_id = {table}.root_id AND sibling.order_id <> {table}.order_id \
         AND (sibling.status <> 'cancelled' OR sibling.updated_clock >= ?1)))"
    )
}

impl Store {
    /// Cancelled orders (and their amends) a purge at `cutoff` (unix
    /// seconds) would remove. Read only.
    pub fn count_expired_orders(&self, cutoff: i64) -> Result<ExpiredOrders, Error> {
        let orders = self.connection.query_row(&format!("SELECT COUNT(*) FROM orders o WHERE {}", expired("o")), params![cutoff], |row| row.get::<_, i64>(0))?;
        let amends = self.connection.query_row(
            &format!("SELECT COUNT(*) FROM order_amends a WHERE EXISTS (SELECT 1 FROM orders o WHERE o.order_id = a.order_id AND {})", expired("o")),
            params![cutoff],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(ExpiredOrders { orders, amends })
    }

    /// Removes at most `limit` expired orders and their amends in one
    /// short transaction (the daemon calls it in a loop, the carnet stays
    /// writable between batches). Returns what this batch removed.
    pub fn purge_expired_orders(&self, cutoff: i64, limit: i64) -> Result<ExpiredOrders, Error> {
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let done = (|| -> Result<ExpiredOrders, rusqlite::Error> {
            let ids: Vec<i64> = {
                let mut select = self.connection.prepare(&format!("SELECT o.order_id FROM orders o WHERE {} ORDER BY o.order_id LIMIT ?2", expired("o")))?;
                let rows = select.query_map(params![cutoff, limit], |row| row.get::<_, i64>(0))?;
                rows.collect::<Result<Vec<i64>, rusqlite::Error>>()?
            };
            let mut removed = ExpiredOrders::default();
            let mut drop_amends = self.connection.prepare("DELETE FROM order_amends WHERE order_id = ?1")?;
            let mut drop_order = self.connection.prepare("DELETE FROM orders WHERE order_id = ?1 AND status = 'cancelled'")?;
            for id in ids {
                removed.amends += drop_amends.execute(params![id])? as i64;
                removed.orders += drop_order.execute(params![id])? as i64;
            }
            Ok(removed)
        })();
        match done {
            Ok(removed) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(removed)
            }
            Err(source) => {
                let _ignored = self.connection.execute_batch("ROLLBACK");
                Err(Error::Sql(format!("purge expired orders: {source}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewOrder, OrderAmend, OrderSide, OrderStatus};

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_800_000_000;

    fn order(account: &str, clock: i64, replaces: Option<i64>) -> NewOrder {
        NewOrder { account: account.to_string(), side: OrderSide::Buy, item_id: 314, quantity: 10, price: 3, dormant: false, clock, replaces, motive: "need".to_string(), ceiling: None, ask_limit: None }
    }

    fn count(store: &Store, table: &str) -> i64 {
        store.connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).expect("count")
    }

    /// Old cancelled rows go with their amends; recent cancelled, open,
    /// matched and settled rows stay; a lineage with a live sibling stays
    /// whole; deals and executions are never touched; the batch size caps
    /// each pass and the passes add up to the dry-run count.
    #[test]
    fn only_finished_old_cancelled_lineages_are_purged() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        crate::ensure_test_item(&store, 314)?;
        let old = NOW - 20 * DAY;
        let recent = NOW - 2 * DAY;
        let cutoff = NOW - 14 * DAY;
        // Twelve old cancelled orders, some with amends.
        let mut doomed = Vec::new();
        for index in 0..12 {
            let id = store.place_order(&order("t01", old, None))?;
            if index % 3 == 0 {
                store.amend_order_in_place(&OrderAmend { order_id: id, price: 5, remaining: None, ceiling: None, ask_limit: None, clock: old + 10 })?;
                store.amend_order_in_place(&OrderAmend { order_id: id, price: 6, remaining: None, ceiling: None, ask_limit: None, clock: old + 20 })?;
            }
            store.cancel_order_because(id, old + 100, "horizon")?;
            doomed.push(id);
        }
        // Kept: cancelled 2 days ago (with an amend), open, matched, settled, and an old lineage whose later row is open.
        let fresh = store.place_order(&order("t02", recent - 100, None))?;
        store.amend_order_in_place(&OrderAmend { order_id: fresh, price: 4, remaining: None, ceiling: None, ask_limit: None, clock: recent - 50 })?;
        store.cancel_order_because(fresh, recent, "need_met")?;
        let open = store.place_order(&order("t03", old, None))?;
        let matched = store.place_order(&order("t04", old, None))?;
        store.set_order_status(matched, OrderStatus::Matched, 5, old + 5)?;
        let settled = store.place_order(&order("t05", old, None))?;
        store.set_order_status(settled, OrderStatus::Matched, 0, old + 5)?;
        store.set_order_status(settled, OrderStatus::Settled, 0, old + 6)?;
        let root = store.place_order(&order("t06", old, None))?;
        store.cancel_order_because(root, old + 50, "amended")?;
        let live_tail = store.place_order(&order("t06", old + 50, Some(root)))?;
        let (before_orders, before_amends) = (count(&store, "orders"), count(&store, "order_amends"));
        let executions_before = count(&store, "executions");
        let deals_before = count(&store, "deals");

        let planned = store.count_expired_orders(cutoff)?;
        assert_eq!(planned.orders, 12, "only the twelve old finished lineages");
        assert_eq!(planned.amends, 8, "four orders with two amends each");
        assert_eq!(count(&store, "orders"), before_orders, "the count deletes nothing");

        let mut removed = ExpiredOrders::default();
        let mut passes = 0;
        loop {
            let batch = store.purge_expired_orders(cutoff, 5)?;
            if batch.orders == 0 {
                break;
            }
            assert!(batch.orders <= 5, "a pass never exceeds its limit");
            removed.orders += batch.orders;
            removed.amends += batch.amends;
            passes += 1;
        }
        assert_eq!((removed, passes), (planned, 3), "the passes add up to the dry run");
        assert_eq!(count(&store, "orders"), before_orders - 12);
        assert_eq!(count(&store, "order_amends"), before_amends - 8);
        for id in &doomed {
            assert!(store.get_order(*id)?.is_none(), "{id} purged");
        }
        for (id, status) in [(fresh, OrderStatus::Cancelled), (open, OrderStatus::Open), (matched, OrderStatus::Matched), (settled, OrderStatus::Settled), (root, OrderStatus::Cancelled), (live_tail, OrderStatus::Open)] {
            assert_eq!(store.get_order(id)?.map(|row| row.status), Some(status), "{id} kept");
        }
        assert_eq!(store.order_open_price(fresh)?, Some(3), "a kept order keeps its amend journal");
        assert_eq!((count(&store, "executions"), count(&store, "deals")), (executions_before, deals_before));
        assert_eq!(store.count_expired_orders(cutoff)?, ExpiredOrders::default(), "nothing left to purge");
        Ok(())
    }
}
