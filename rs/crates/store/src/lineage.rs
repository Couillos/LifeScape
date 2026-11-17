#![forbid(unsafe_code)]
//! Order lineage (plan marché M0/M1, decision Q1 2026-11-16).
//!
//! An amend is a cancel + a new row (the market loses the old rank): it
//! used to count as a cancellation, and the age of an offer restarted at
//! every walk step. Schema 6 links each replacement to the row it
//! replaced (`replaces`), every row to the first order of its lineage
//! (`root_id`) and its opening clock (`root_clock`, t0), and keeps the
//! order's `motive` and the `cancel_reason` of a closed row (`amended`
//! for a replaced one). Statuses are unchanged (Q1).
//!
//! History migration: a `cancelled` row replaced by the next row of the
//! same account, object and side created within `AMEND_WINDOW_SECS` of
//! its cancel is an amend (the live amend writes both rows in the same
//! second). Ids are processed in ascending order, so a lineage's root is
//! known before its replacements.

use std::collections::HashMap;

use crate::Error;

/// Seconds between a cancel and the replacement that makes it an amend.
pub const AMEND_WINDOW_SECS: i64 = 1;

/// Columns and indexes of schema 6 (the reclassification runs after).
pub(crate) const MIGRATION_V6_COLUMNS: &str = "
ALTER TABLE orders ADD COLUMN replaces INTEGER;
ALTER TABLE orders ADD COLUMN root_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE orders ADD COLUMN root_clock INTEGER NOT NULL DEFAULT 0;
ALTER TABLE orders ADD COLUMN motive TEXT NOT NULL DEFAULT '';
ALTER TABLE orders ADD COLUMN cancel_reason TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_orders_root ON orders(root_id);
CREATE INDEX IF NOT EXISTS idx_orders_replaces ON orders(replaces);
";

/// One stored row as the reclassification reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct LineageRow {
    pub order_id: i64,
    pub account: String,
    pub side: String,
    pub item_id: i64,
    pub cancelled: bool,
    pub created_clock: i64,
    pub updated_clock: i64,
}

/// Lineage of one row after the reclassification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineageLink {
    pub order_id: i64,
    pub replaces: Option<i64>,
    pub root_id: i64,
    pub root_clock: i64,
}

/// Pure reclassification over rows sorted by ascending id: each row
/// claims at most one earlier cancelled row of the same account, object
/// and side whose cancel clock is within `AMEND_WINDOW_SECS` of its own
/// creation (the closest, then the latest id); a claimed row is never
/// claimed twice.
pub fn link_amends(rows: &[LineageRow]) -> Vec<LineageLink> {
    let mut candidates: HashMap<(&str, &str, i64), Vec<(i64, i64)>> = HashMap::new();
    let mut lineage: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let key = (row.account.as_str(), row.side.as_str(), row.item_id);
        let mut replaces = None;
        if let Some(waiting) = candidates.get_mut(&key) {
            // Creation clocks only grow with ids: a cancel older than the
            // window can never be claimed again.
            waiting.retain(|(_, cancelled_at)| *cancelled_at >= row.created_clock - AMEND_WINDOW_SECS);
            let best = waiting
                .iter()
                .enumerate()
                .filter(|(_, (_, cancelled_at))| (cancelled_at - row.created_clock).abs() <= AMEND_WINDOW_SECS)
                .min_by_key(|(_, (order_id, cancelled_at))| ((cancelled_at - row.created_clock).abs(), -order_id))
                .map(|(index, _)| index);
            if let Some(index) = best {
                replaces = Some(waiting.remove(index).0);
            }
        }
        let (root_id, root_clock) = replaces.and_then(|previous| lineage.get(&previous).copied()).unwrap_or((row.order_id, row.created_clock));
        lineage.insert(row.order_id, (root_id, root_clock));
        out.push(LineageLink { order_id: row.order_id, replaces, root_id, root_clock });
        if row.cancelled {
            candidates.entry(key).or_default().push((row.order_id, row.updated_clock));
        }
    }
    out
}

/// Counts of one history reclassification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReclassifyCounts {
    pub rows: usize,
    pub amends: usize,
}

/// Runs inside the schema-6 transaction: every row gets its lineage,
/// every replaced row the `amended` cancel reason.
pub(crate) fn reclassify_history(connection: &rusqlite::Connection) -> Result<ReclassifyCounts, Error> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT order_id, account, side, item_id, status, created_clock, updated_clock FROM orders ORDER BY order_id",
        )?;
        let mapped = statement.query_map([], |row| {
            Ok(LineageRow {
                order_id: row.get(0)?,
                account: row.get(1)?,
                side: row.get(2)?,
                item_id: row.get(3)?,
                cancelled: row.get::<_, String>(4)? == "cancelled",
                created_clock: row.get(5)?,
                updated_clock: row.get(6)?,
            })
        })?;
        mapped.collect::<Result<Vec<LineageRow>, rusqlite::Error>>()?
    };
    let links = link_amends(&rows);
    connection.execute_batch("UPDATE orders SET root_id = order_id, root_clock = created_clock")?;
    let mut amends = 0usize;
    {
        let mut link_row = connection.prepare("UPDATE orders SET replaces = ?1, root_id = ?2, root_clock = ?3 WHERE order_id = ?4")?;
        let mut mark_amended = connection.prepare("UPDATE orders SET cancel_reason = ?1 WHERE order_id = ?2")?;
        for link in links.iter().filter(|link| link.replaces.is_some()) {
            link_row.execute(rusqlite::params![link.replaces, link.root_id, link.root_clock, link.order_id])?;
            mark_amended.execute(rusqlite::params![crate::orders::AMENDED_REASON, link.replaces])?;
            amends += 1;
        }
    }
    Ok(ReclassifyCounts { rows: rows.len(), amends })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{temp_database_path, OrderStatus, Store};

    fn row(order_id: i64, account: &str, side: &str, cancelled: bool, created_clock: i64, updated_clock: i64) -> LineageRow {
        LineageRow { order_id, account: account.to_string(), side: side.to_string(), item_id: 314, cancelled, created_clock, updated_clock }
    }

    /// A walk of three steps is one lineage; a cancel followed by a new
    /// bid 5 s later is a true cancel and a new lineage; another side or
    /// account never joins.
    #[test]
    fn amends_link_within_one_second_only() {
        let rows = vec![
            row(1, "a", "buy", true, 100, 2100),
            row(2, "a", "buy", true, 2100, 4101),
            row(3, "a", "buy", false, 4100, 4100),
            row(4, "a", "buy", true, 5000, 6000),
            row(5, "a", "buy", false, 6005, 6005),
            row(6, "a", "sell", false, 6000, 6000),
            row(7, "b", "buy", false, 6000, 6000),
        ];
        let links = link_amends(&rows);
        let shape: Vec<(i64, Option<i64>, i64, i64)> = links.iter().map(|link| (link.order_id, link.replaces, link.root_id, link.root_clock)).collect();
        assert_eq!(
            shape,
            vec![(1, None, 1, 100), (2, Some(1), 1, 100), (3, Some(2), 1, 100), (4, None, 4, 5000), (5, None, 5, 6005), (6, None, 6, 6000), (7, None, 7, 6000)]
        );
    }

    /// Golden migration on a fabricated schema-5 extract (live shapes,
    /// 2026-11-16): ryan_mason's vambraces walk (amends at the same
    /// second), a true cancel then a lower repose 3 s later, a settled
    /// seller untouched. Nothing lost, statuses unchanged.
    #[test]
    fn v5_extract_migrates_to_lineages() -> Result<(), Error> {
        use rusqlite::Connection;
        let path = temp_database_path("migrate-v5");
        let _remove_guard = RemoveGuard { path: path.clone() };
        {
            let connection = Connection::open(&path)?;
            for step in [crate::schema::MIGRATION_V1, crate::schema::MIGRATION_V2, crate::schema::MIGRATION_V3, crate::schema::MIGRATION_V4, crate::schema::MIGRATION_V5] {
                connection.execute_batch(step)?;
            }
            connection.execute_batch(
                "INSERT INTO items(item_id, name, shop_price, alch_value) VALUES (1063, 'Leather vambraces', 18, 7);
                 INSERT INTO orders(order_id, account, side, item_id, quantity, remaining, price, status, dormant, created_clock, updated_clock) VALUES
                   (10, 'ryan_mason', 'buy', 1063, 1, 1, 10, 'cancelled', 0, 1000, 1120),
                   (11, 'ryan_mason', 'buy', 1063, 1, 1, 11, 'cancelled', 0, 1120, 1241),
                   (12, 'ryan_mason', 'buy', 1063, 1, 1, 12, 'cancelled', 0, 1240, 1400),
                   (13, 'ryan_mason', 'buy', 1063, 1, 1, 10, 'open', 0, 1403, 1403),
                   (14, 'fiona_park', 'sell', 1063, 3, 0, 9, 'settled', 0, 1120, 1300);",
            )?;
        }
        let store = Store::open(&path)?;
        assert_eq!(store.schema_version()?, crate::schema::SCHEMA_VERSION);
        assert!(crate::schema::SCHEMA_VERSION >= 6, "v6 lineage columns present");
        let shape = |order_id: i64| -> Result<(Option<i64>, i64, i64, String, OrderStatus), Error> {
            let found = store.get_order(order_id)?.ok_or_else(|| Error::NotFound(format!("order {order_id}")))?;
            Ok((found.replaces, found.root_id, found.root_clock, found.cancel_reason, found.status))
        };
        assert_eq!(shape(10)?, (None, 10, 1000, "amended".to_string(), OrderStatus::Cancelled));
        assert_eq!(shape(11)?, (Some(10), 10, 1000, "amended".to_string(), OrderStatus::Cancelled));
        assert_eq!(shape(12)?, (Some(11), 10, 1000, String::new(), OrderStatus::Cancelled), "the walk's last row: a true cancel");
        assert_eq!(shape(13)?, (None, 13, 1403, String::new(), OrderStatus::Open), "3 s later: a new, lower lineage");
        assert_eq!(shape(14)?, (None, 14, 1120, String::new(), OrderStatus::Settled), "the other side is its own lineage");
        drop(store);
        let reopened = Store::open(&path)?;
        assert_eq!(reopened.get_order(11)?.map(|found| found.root_id), Some(10), "idempotent reopen");
        Ok(())
    }

    struct RemoveGuard {
        path: std::path::PathBuf,
    }

    impl Drop for RemoveGuard {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_file(&self.path);
        }
    }
}
