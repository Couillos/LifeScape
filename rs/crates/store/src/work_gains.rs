#![forbid(unsafe_code)]
//! What each account's work really moved over a window (plan équilibre
//! step 1, decision D1): the contract-2 task reports (`measure_version`
//! 2) created since a clock, their `work_ticks` summed per account and
//! their signed flows (`task_flows`) netted per account and item. The
//! orchestrator values the flows at the current price and divides by the
//! work ticks (`time_value`): the coins one tick of a bot's work really
//! earned. Raw quantities only here, no price and no rule.

use std::collections::BTreeMap;

use rusqlite::params;

use crate::measures::MEASURE_VERSION;
use crate::{Error, Store};

/// One account's contract-2 work in a window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountWork {
    pub account: String,
    /// Work ticks of its contract-2 reports (pauses excluded).
    pub work_ticks: i64,
    /// Net signed quantity per item display name over the flows whose
    /// cause the caller values, zero nets left out, by name.
    pub flows: Vec<(String, i64)>,
}

/// Work ticks per account of the window (`?1` measure version, `?2`
/// first clock). The unary `+` keeps the measure-version index out: the
/// window is read through the clock index (`schema::REPORT_CLOCK_INDEX`),
/// never through every contract-2 report ever stored.
pub(crate) const WORK_TICKS_SQL: &str =
    "SELECT account, SUM(work_ticks) FROM task_reports WHERE +measure_version >= ?1 AND created_clock >= ?2 GROUP BY account";

/// Flows per account, item and cause of the window (same parameters):
/// driven by the window's reports (clock index, as `WORK_TICKS_SQL`),
/// then each one's flows by their report index, never a scan of every
/// report or flow ever stored.
pub(crate) const WORK_FLOWS_SQL: &str = "SELECT r.account, f.item_name, f.cause, SUM(f.quantity) FROM task_reports r CROSS JOIN task_flows f ON f.report_id = r.report_id \
     WHERE +r.measure_version >= ?1 AND r.created_clock >= ?2 GROUP BY r.account, f.item_name, f.cause";

impl Store {
    /// Clock of the newest task report, `None` on an empty store: the
    /// end of a copy's window (an offline read of an old copy).
    pub fn newest_report_clock(&self) -> Result<Option<i64>, Error> {
        Ok(self.connection.query_row("SELECT MAX(created_clock) FROM task_reports", [], |row| row.get::<_, Option<i64>>(0))?)
    }

    /// Work of every account whose contract-2 reports were created at or
    /// after `since` (module doc), by account name. Flows of a cause in
    /// `unvalued_causes` are left out; an account with work and no valued
    /// flow reads an empty `flows`.
    pub fn work_flows_since(&self, since: i64, unvalued_causes: &[&str]) -> Result<Vec<AccountWork>, Error> {
        let mut works: BTreeMap<String, (i64, BTreeMap<String, i64>)> = BTreeMap::new();
        let mut statement = self.connection.prepare(WORK_TICKS_SQL)?;
        let rows = statement.query_map(params![MEASURE_VERSION, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        for row in rows {
            let (account, work_ticks) = row?;
            works.entry(account).or_default().0 += work_ticks;
        }
        let mut statement = self.connection.prepare(WORK_FLOWS_SQL)?;
        let rows = statement.query_map(params![MEASURE_VERSION, since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
        })?;
        for row in rows {
            let (account, item, cause, quantity) = row?;
            if unvalued_causes.contains(&cause.as_str()) {
                continue;
            }
            *works.entry(account).or_default().1.entry(item).or_insert(0) += quantity;
        }
        Ok(works
            .into_iter()
            .map(|(account, (work_ticks, flows))| AccountWork { account, work_ticks, flows: flows.into_iter().filter(|(_, quantity)| *quantity != 0).collect() })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::AccountWork;
    use crate::{Error, NewTaskReport, ReportMeasure, Store};

    fn report(store: &Store, account: &str, clock: i64, work_ticks: Option<i64>, flows: &[(&str, &str, i64)]) -> Result<i64, Error> {
        let report_id = store.record_task_report(&NewTaskReport {
            account: account.to_string(),
            node_id: "n1".to_string(),
            method: "woodcut".to_string(),
            script: "woodcut".to_string(),
            ticks: 700,
            stop: "goal_reached".to_string(),
            context_level: 1,
            clock,
        })?;
        if let Some(work_ticks) = work_ticks {
            let flows = flows.iter().map(|(item, cause, quantity)| (item.to_string(), cause.to_string(), *quantity)).collect();
            store.record_report_measure(report_id, &ReportMeasure { work_ticks, flows, ..ReportMeasure::default() })?;
        }
        Ok(report_id)
    }

    /// The window's contract-2 reports only, work summed, flows netted
    /// per item over the valued causes; a measure-1 report and an older
    /// one never count.
    #[test]
    fn work_flows_net_the_window_per_account() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        report(&store, "ann", 50, Some(9000), &[("Logs", "product", 500)])?;
        report(&store, "ann", 100, Some(600), &[("Logs", "product", 20), ("Coins", "shop_sold", 160), ("Logs", "shop_sold", -10), ("Coins", "traded", 999)])?;
        report(&store, "ann", 120, Some(400), &[("Oak logs", "product", 5), ("Coins", "unattributed", 77), ("Bronze axe", "shop_bought", 1), ("Coins", "shop_bought", -16)])?;
        report(&store, "ann", 130, None, &[])?;
        report(&store, "bob", 110, Some(300), &[("Logs", "traded", -4)])?;
        report(&store, "cid", 140, Some(0), &[("Bones", "loot", 3), ("Bones", "input", -3)])?;
        let found = store.work_flows_since(100, &["traded", "unattributed"])?;
        let expected = vec![
            AccountWork {
                account: "ann".to_string(),
                work_ticks: 1000,
                flows: vec![("Bronze axe".to_string(), 1), ("Coins".to_string(), 144), ("Logs".to_string(), 10), ("Oak logs".to_string(), 5)],
            },
            AccountWork { account: "bob".to_string(), work_ticks: 300, flows: Vec::new() },
            AccountWork { account: "cid".to_string(), work_ticks: 0, flows: Vec::new() },
        ];
        assert_eq!(found, expected);
        Ok(())
    }

    /// The window reads the reports through their clock index (review of
    /// lot 1: `task_reports` is never purged, the measure runs every 5
    /// minutes under the store's lock): never a scan of every report.
    #[test]
    fn the_window_reads_through_the_clock_index() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        for sql in [super::WORK_TICKS_SQL, super::WORK_FLOWS_SQL] {
            let mut statement = store.connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let steps = statement.query_map(rusqlite::params![2, 0], |row| row.get::<_, String>(3))?.collect::<Result<Vec<String>, _>>()?;
            let reports = steps.iter().find(|step| step.contains(" r ") || step.contains("task_reports")).cloned().unwrap_or_default();
            assert!(reports.contains(crate::schema::REPORT_CLOCK_INDEX), "{sql}: {steps:?}");
        }
        Ok(())
    }

    /// The newest report's clock ends a copy's window; none on an empty
    /// store.
    #[test]
    fn the_newest_report_clock_ends_a_copy_window() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        assert_eq!(store.newest_report_clock()?, None);
        report(&store, "ann", 130, Some(600), &[])?;
        report(&store, "bob", 50, None, &[])?;
        assert_eq!(store.newest_report_clock()?, Some(130));
        Ok(())
    }

    /// Nothing in the window: no account.
    #[test]
    fn an_empty_window_reads_no_account() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        report(&store, "ann", 50, Some(9000), &[("Logs", "product", 500)])?;
        assert!(store.work_flows_since(100, &[])?.is_empty());
        Ok(())
    }
}
