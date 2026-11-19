#![forbid(unsafe_code)]
//! Measures of the self-balancing market (docs/plan-equilibre.txt, step 0,
//! lot 0): what `orchestrator economy` prints in its "équilibre" section.
//! Staff accounts are left out (`excluded`).
//!
//! - unsold stock: units resting in sell orders now (open or matched,
//!   remaining, not dormant) beside the units each object traded in the
//!   window (`executions`), so the caller reads days of stock; the fleet
//!   pace is every execution of the window (`window_traded_units`), sold
//!   out objects included;
//! - quantities are units: `orders.remaining` and `executions.quantity`
//!   count units, and lot quotation is off (orchestrator
//!   `market_units::LOT_QUOTATION` false, every lot is 1), so no lot
//!   conversion is made here;
//! - fleet work: `task_reports.work_ticks` of the window summed by
//!   (planner method, combat style, node), so the caller maps methods to
//!   skills and filler nodes with the planner's and brain's own helpers.

use std::collections::BTreeMap;

use rusqlite::params;

use crate::market_criteria::excluded_account;
use crate::{Error, Store};

/// One object's resting asks and its traded units over a window.
#[derive(Debug, Clone, PartialEq)]
pub struct UnsoldStock {
    pub item_id: i64,
    pub name: String,
    /// Units resting in sell orders now (staff left out).
    pub resting_units: i64,
    /// Units the object traded in the window (executions).
    pub traded_units: i64,
}

impl UnsoldStock {
    /// Days the resting units last at the window's trading pace
    /// (`span_secs` = the window's length); `None` without a trade.
    pub fn days(&self, span_secs: i64) -> Option<f64> {
        stock_days(self.resting_units, self.traded_units, span_secs)
    }
}

/// Days `resting` units last when `traded` units sold in `span_secs`
/// (`None` without a trade or a span).
pub fn stock_days(resting: i64, traded: i64, span_secs: i64) -> Option<f64> {
    if traded <= 0 || span_secs <= 0 {
        return None;
    }
    let per_day = traded as f64 * 86_400.0 / span_secs as f64;
    Some(resting as f64 / per_day)
}

/// Work ticks of one (planner method, combat style, node) in a window.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodWork {
    pub planner_method: String,
    pub combat_style: String,
    pub node_id: String,
    pub work_ticks: i64,
}

impl Store {
    /// Every object with resting asks (staff left out), its resting units
    /// and the units it traded in `[since, until)` (`until` 0 = now), by
    /// item id.
    pub fn unsold_stock(&self, since: i64, until: i64, excluded: &[String]) -> Result<Vec<UnsoldStock>, Error> {
        let mut asks = self.connection.prepare(
            "SELECT o.account, o.item_id, COALESCE(i.name, ''), SUM(o.remaining) FROM orders o LEFT JOIN items i ON i.item_id = o.item_id \
             WHERE o.side = 'sell' AND o.status IN ('open','matched') AND o.remaining > 0 AND o.dormant = 0 GROUP BY o.account, o.item_id",
        )?;
        let rows = asks.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?)))?;
        let mut by_item: BTreeMap<i64, UnsoldStock> = BTreeMap::new();
        for row in rows {
            let (account, item_id, name, units) = row?;
            if excluded_account(excluded, &account) {
                continue;
            }
            by_item.entry(item_id).or_insert_with(|| UnsoldStock { item_id, name, resting_units: 0, traded_units: 0 }).resting_units += units;
        }
        let mut trades = self.connection.prepare("SELECT item_id, SUM(quantity) FROM executions WHERE clock >= ?1 AND (?2 <= 0 OR clock < ?2) GROUP BY item_id")?;
        let traded = trades.query_map(params![since, until], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        for row in traded {
            let (item_id, units) = row?;
            if let Some(stock) = by_item.get_mut(&item_id) {
                stock.traded_units = units;
            }
        }
        Ok(by_item.into_values().collect())
    }

    /// Units of every execution in `[since, until)` (`until` 0 = now),
    /// objects sold out since included: the fleet's trading pace.
    pub fn window_traded_units(&self, since: i64, until: i64) -> Result<i64, Error> {
        Ok(self.connection.query_row(
            "SELECT COALESCE(SUM(quantity), 0) FROM executions WHERE clock >= ?1 AND (?2 <= 0 OR clock < ?2)",
            params![since, until],
            |row| row.get::<_, i64>(0),
        )?)
    }

    /// Work ticks of the task reports created in `[since, until)` (`until`
    /// 0 = now), summed by (planner method, combat style, node), staff
    /// left out; reports without a planner method keep an empty one.
    pub fn work_by_method(&self, since: i64, until: i64, excluded: &[String]) -> Result<Vec<MethodWork>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT account, planner_method, combat_style, node_id, SUM(work_ticks) FROM task_reports \
             WHERE created_clock >= ?1 AND (?2 <= 0 OR created_clock < ?2) GROUP BY account, planner_method, combat_style, node_id",
        )?;
        let rows = statement.query_map(params![since, until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?))
        })?;
        let mut grouped: BTreeMap<(String, String, String), i64> = BTreeMap::new();
        for row in rows {
            let (account, planner_method, combat_style, node_id, work_ticks) = row?;
            if excluded_account(excluded, &account) {
                continue;
            }
            *grouped.entry((planner_method, combat_style, node_id)).or_insert(0) += work_ticks;
        }
        Ok(grouped.into_iter().map(|((planner_method, combat_style, node_id), work_ticks)| MethodWork { planner_method, combat_style, node_id, work_ticks }).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{stock_days, MethodWork, UnsoldStock};
    use crate::{Error, NewExecution, NewOrder, NewTaskReport, OrderSide, OrderStatus, ReportMeasure, Store};

    const YEW: i64 = 1515;
    const BEEF: i64 = 2132;
    const BAR: i64 = 2359;

    fn order(account: &str, side: OrderSide, item_id: i64, quantity: i64, clock: i64) -> NewOrder {
        NewOrder { account: account.to_string(), side, item_id, quantity, price: 10, clock, ..NewOrder::default() }
    }

    fn report(store: &Store, account: &str, node_id: &str, planner_method: &str, combat_style: &str, work_ticks: i64, clock: i64) -> Result<(), Error> {
        let report_id = store.record_task_report(&NewTaskReport {
            account: account.to_string(),
            node_id: node_id.to_string(),
            method: "legacy".to_string(),
            script: "script".to_string(),
            ticks: work_ticks + 10,
            stop: "goal_reached".to_string(),
            context_level: 1,
            clock,
        })?;
        let measure = ReportMeasure { planner_method: planner_method.to_string(), combat_style: combat_style.to_string(), work_ticks, ..ReportMeasure::default() };
        store.record_report_measure(report_id, &measure)
    }

    /// Lot 0: resting asks now (staff, bids, filled and cancelled rows
    /// out) beside the units traded in the window (executions before it
    /// out); an object with asks and no trade keeps 0 traded units.
    #[test]
    fn unsold_stock_reads_resting_asks_and_window_trades() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        for (item_id, name) in [(YEW, "Yew logs"), (BEEF, "Raw beef"), (BAR, "Mithril bar")] {
            store.upsert_item(&crate::Item { item_id, name: name.to_string(), shop_price: 2, alch_value: 1 })?;
        }
        let since = 10_000;
        store.place_order(&order("cutter", OrderSide::Sell, YEW, 900, 100))?;
        store.place_order(&order("other", OrderSide::Sell, YEW, 300, since + 5))?;
        store.place_order(&order("admin", OrderSide::Sell, YEW, 5_000, since + 6))?;
        store.place_order(&order("buyer", OrderSide::Buy, YEW, 700, since + 7))?;
        store.place_order(&order("farmer", OrderSide::Sell, BEEF, 40, since + 8))?;
        let gone = store.place_order(&order("smith", OrderSide::Sell, BAR, 50, since + 9))?;
        store.set_order_status(gone, OrderStatus::Settled, 0, since + 20)?;
        let cancelled = store.place_order(&order("smith", OrderSide::Sell, BAR, 30, since + 30))?;
        store.set_order_status(cancelled, OrderStatus::Cancelled, 30, since + 40)?;
        for (item_id, quantity, clock) in [(YEW, 100, since + 50), (YEW, 50, since + 60), (YEW, 999, since - 1), (BAR, 50, since + 20)] {
            store.record_execution(&NewExecution { item_id, quantity, price: 10, clock, server_tick: 0, taker_side: None })?;
        }
        let stock = store.unsold_stock(since, 0, &["admin".to_string()])?;
        assert_eq!(
            stock,
            vec![
                UnsoldStock { item_id: YEW, name: "Yew logs".to_string(), resting_units: 1_200, traded_units: 150 },
                UnsoldStock { item_id: BEEF, name: "Raw beef".to_string(), resting_units: 40, traded_units: 0 },
            ],
            "staff asks, bids, settled and cancelled asks out; the older execution out"
        );
        let day = 86_400;
        assert_eq!(stock[0].days(day / 2), Some(4.0), "150 units in half a day = 300 a day: 1200 units last 4 days");
        assert_eq!(stock[1].days(day), None, "no trade: no pace");
        assert_eq!(stock_days(10, 5, 0), None);
        assert_eq!(store.window_traded_units(since, 0)?, 200, "the 50 mithril bars sold out (no ask left) still count in the fleet's pace");
        assert_eq!(store.window_traded_units(since, since + 55)?, 150);
        Ok(())
    }

    /// Lot 0: work ticks of the window by (planner method, combat style,
    /// node), staff and reports outside the window left out.
    #[test]
    fn work_by_method_sums_the_window_work_ticks() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let since = 10_000;
        report(&store, "cutter", "fill-n4", "gather:woodcut/yew_logs", "", 600, since + 1)?;
        report(&store, "cutter", "fill-n4", "gather:woodcut/yew_logs", "", 400, since + 2)?;
        report(&store, "cutter", "n9", "gather:woodcut/yew_logs", "", 300, since + 3)?;
        report(&store, "fighter", "n2", "fight:melee/men", "melee:strength", 200, since + 4)?;
        report(&store, "Admin", "n3", "smelt:steel_bar", "", 9_000, since + 5)?;
        report(&store, "cutter", "n9", "gather:woodcut/yew_logs", "", 7_000, since - 1)?;
        let mut work = store.work_by_method(since, 0, &["admin".to_string()])?;
        work.sort_by(|left, right| (&left.planner_method, &left.node_id).cmp(&(&right.planner_method, &right.node_id)));
        let row = |method: &str, style: &str, node: &str, ticks: i64| MethodWork { planner_method: method.to_string(), combat_style: style.to_string(), node_id: node.to_string(), work_ticks: ticks };
        assert_eq!(
            work,
            vec![row("fight:melee/men", "melee:strength", "n2", 200), row("gather:woodcut/yew_logs", "", "fill-n4", 1_000), row("gather:woodcut/yew_logs", "", "n9", 300)],
            "staff (any case) and the report before the window out"
        );
        Ok(())
    }
}
