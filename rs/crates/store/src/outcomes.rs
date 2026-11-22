#![forbid(unsafe_code)]
//! Read-only outcome counts for the economy page: exchanges (deals) and
//! offers (orders) grouped by final status and reason, one pass for
//! several time windows.
//!
//! An offer is counted once per real order, not once per price change:
//! a chain of rows linked by `replaces`/`root_id` keeps `cancelled` +
//! `cancel_reason = 'amended'` on every row but the last, so only the rows
//! whose reason is not `amended` are counted (the chain's final row).
//! Robust to in-place updates too (no `amended` rows then).

use crate::{Error, Store};

/// One (status, reason) group with its count per requested window.
#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeCount {
    pub status: String,
    pub reason: String,
    /// One count per `since` bound passed, same order.
    pub per_window: Vec<i64>,
}

/// Which table and columns one outcome query reads.
struct OutcomeQuery<'a> {
    table: &'a str,
    status_column: &'a str,
    reason_column: &'a str,
    clock_expression: &'a str,
    account_columns: &'a [&'a str],
    extra_filter: &'a str,
}

impl Store {
    /// Deals by (status, fail_reason), counted from `created_clock`;
    /// a deal with an excluded buyer or seller is skipped.
    pub fn deal_outcomes(&self, since: &[i64], excluded: &[String]) -> Result<Vec<OutcomeCount>, Error> {
        self.outcome_rows(
            &OutcomeQuery {
                table: "deals",
                status_column: "status",
                reason_column: "fail_reason",
                clock_expression: "created_clock",
                account_columns: &["buyer", "seller"],
                extra_filter: "",
            },
            since,
            excluded,
        )
    }

    /// Real orders by (status, cancel_reason), dated from the chain's
    /// origin (`root_clock`, else `created_clock`); `amended` rows skipped.
    pub fn order_outcomes(&self, since: &[i64], excluded: &[String]) -> Result<Vec<OutcomeCount>, Error> {
        self.outcome_rows(
            &OutcomeQuery {
                table: "orders",
                status_column: "status",
                reason_column: "cancel_reason",
                clock_expression: "(CASE WHEN root_clock > 0 THEN root_clock ELSE created_clock END)",
                account_columns: &["account"],
                extra_filter: "cancel_reason != 'amended'",
            },
            since,
            excluded,
        )
    }

    fn outcome_rows(
        &self,
        query: &OutcomeQuery<'_>,
        since: &[i64],
        excluded: &[String],
    ) -> Result<Vec<OutcomeCount>, Error> {
        let windows = since.len();
        let clock = query.clock_expression;
        let sums: String = (1..=windows).map(|index| format!(", COALESCE(SUM({clock} >= ?{index}), 0)")).collect();
        let marks: Vec<String> = (0..excluded.len()).map(|index| format!("?{}", windows + 1 + index)).collect();
        let mut filters: Vec<String> = Vec::new();
        if !query.extra_filter.is_empty() {
            filters.push(query.extra_filter.to_string());
        }
        if !marks.is_empty() {
            let list = marks.join(", ");
            for column in query.account_columns {
                filters.push(format!("{column} NOT IN ({list})"));
            }
        }
        let where_clause = if filters.is_empty() { String::new() } else { format!(" WHERE {}", filters.join(" AND ")) };
        let sql = format!(
            "SELECT {status}, {reason}{sums} FROM {table}{where_clause} GROUP BY {status}, {reason}",
            status = query.status_column,
            reason = query.reason_column,
            table = query.table,
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut parameters: Vec<rusqlite::types::Value> = since.iter().map(|bound| (*bound).into()).collect();
        parameters.extend(excluded.iter().map(|name| name.clone().into()));
        let mut rows = statement.query(rusqlite::params_from_iter(parameters))?;
        let mut counts = Vec::new();
        while let Some(row) = rows.next()? {
            let mut per_window = Vec::with_capacity(windows);
            for index in 0..windows {
                per_window.push(row.get::<_, i64>(2 + index)?);
            }
            counts.push(OutcomeCount { status: row.get(0)?, reason: row.get(1)?, per_window });
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{NewOrder, OrderSide, OrderStatus};

    fn order(account: &str, clock: i64, replaces: Option<i64>) -> NewOrder {
        NewOrder {
            account: account.to_string(),
            side: OrderSide::Buy,
            item_id: 995,
            quantity: 1,
            price: 10,
            clock,
            replaces,
            ..NewOrder::default()
        }
    }

    fn count(rows: &[OutcomeCount], status: &str, reason: &str, window: usize) -> i64 {
        rows.iter().find(|row| row.status == status && row.reason == reason).map_or(0, |row| row.per_window[window])
    }

    #[test]
    fn an_amended_chain_counts_once_by_its_final_row() {
        let store = Store::open_in_memory().expect("store");
        crate::ensure_test_item(&store, 995).expect("item");
        // Chain of three rows ending cancelled for need_met.
        let first = store.place_order(&order("t01", 1000, None)).expect("first");
        store.cancel_order_because(first, 1010, "amended").expect("amend 1");
        let second = store.place_order(&order("t01", 1010, Some(first))).expect("second");
        store.cancel_order_because(second, 1020, "amended").expect("amend 2");
        let third = store.place_order(&order("t01", 1020, Some(second))).expect("third");
        store.cancel_order_because(third, 1030, "need_met").expect("final cancel");
        // Chain settled after one reprice.
        let fourth = store.place_order(&order("t02", 1100, None)).expect("fourth");
        store.cancel_order_because(fourth, 1110, "amended").expect("amend 3");
        let fifth = store.place_order(&order("t02", 1110, Some(fourth))).expect("fifth");
        store.set_order_status(fifth, OrderStatus::Settled, 0, 1120).expect("settled");
        // Open order, never repriced; staff order ignored.
        store.place_order(&order("t03", 1200, None)).expect("open");
        store.place_order(&order("admin", 1200, None)).expect("staff");

        let excluded = vec!["admin".to_string()];
        let rows = store.order_outcomes(&[0, 1150], &excluded).expect("outcomes");
        assert_eq!(count(&rows, "cancelled", "need_met", 0), 1);
        assert_eq!(count(&rows, "settled", "", 0), 1);
        assert_eq!(count(&rows, "open", "", 0), 1);
        assert_eq!(count(&rows, "cancelled", "amended", 0), 0, "amended rows are not outcomes");
        // Window since 1150: only the open order, dated by its origin.
        assert_eq!(count(&rows, "open", "", 1), 1);
        assert_eq!(count(&rows, "cancelled", "need_met", 1), 0);
        assert_eq!(count(&rows, "settled", "", 1), 0);
    }

    #[test]
    fn a_chain_dates_from_its_origin_not_its_last_reprice() {
        let store = Store::open_in_memory().expect("store");
        crate::ensure_test_item(&store, 995).expect("item");
        let first = store.place_order(&order("t01", 1000, None)).expect("first");
        store.cancel_order_because(first, 5000, "amended").expect("amend");
        store.place_order(&order("t01", 5000, Some(first))).expect("second");
        let rows = store.order_outcomes(&[2000], &[]).expect("outcomes");
        assert_eq!(count(&rows, "open", "", 0), 0, "origin at 1000 is before the window");
    }
}
