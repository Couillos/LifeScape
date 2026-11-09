#![forbid(unsafe_code)]
//! Task reports (raw, one per finished engagement) and yield lines.
//! Aggregates always derive from `task_yields`, never stored twice.

use crate::{Error, Store};

/// Caller-supplied fields for one finished task engagement.
#[derive(Clone, Debug, PartialEq)]
pub struct NewTaskReport {
    pub account: String,
    pub node_id: String,
    pub method: String,
    pub script: String,
    pub ticks: i64,
    pub stop: String,
    pub context_level: i64,
    pub clock: i64,
}

/// Stored report row.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskReport {
    pub report_id: i64,
    pub account: String,
    pub node_id: String,
    pub method: String,
    pub script: String,
    pub ticks: i64,
    pub stop: String,
    pub context_level: i64,
    pub created_clock: i64,
}

/// Caller-supplied yield line: either an item flow (`item_name` set) or an
/// xp gain (`skill` set). Both set is allowed, neither is rejected.
#[derive(Clone, Debug, PartialEq)]
pub struct NewTaskYield {
    pub report_id: i64,
    pub item_name: String,
    pub provenance: String,
    pub quantity: i64,
    pub skill: String,
    pub xp: i64,
}

/// Stored yield row.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskYield {
    pub yield_id: i64,
    pub report_id: i64,
    pub item_name: String,
    pub provenance: String,
    pub quantity: i64,
    pub skill: String,
    pub xp: i64,
}

/// Summed quantities grouped by method, item and provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct YieldAggregate {
    pub method: String,
    pub item_name: String,
    pub provenance: String,
    pub total_quantity: i64,
    pub total_xp: i64,
    pub report_count: i64,
}

fn map_yield(row: &rusqlite::Row<'_>) -> Result<TaskYield, rusqlite::Error> {
    Ok(TaskYield {
        yield_id: row.get(0)?,
        report_id: row.get(1)?,
        item_name: row.get(2)?,
        provenance: row.get(3)?,
        quantity: row.get(4)?,
        skill: row.get(5)?,
        xp: row.get(6)?,
    })
}

impl Store {
    /// Appends a raw task report, returns its id.
    pub fn record_task_report(&self, new_report: &NewTaskReport) -> Result<i64, Error> {
        if new_report.account.is_empty() || new_report.method.is_empty() {
            return Err(Error::Invalid("report account/method is empty".to_string()));
        }
        if new_report.ticks < 0 {
            return Err(Error::Invalid(format!("bad report ticks {}", new_report.ticks)));
        }
        self.connection.execute(
            "INSERT INTO task_reports(account, node_id, method, script, ticks, stop, \
             context_level, created_clock) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                new_report.account,
                new_report.node_id,
                new_report.method,
                new_report.script,
                new_report.ticks,
                new_report.stop,
                new_report.context_level,
                new_report.clock,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Appends one yield line to an existing report.
    pub fn record_task_yield(&self, new_yield: &NewTaskYield) -> Result<i64, Error> {
        if new_yield.provenance.is_empty() {
            return Err(Error::Invalid("yield provenance is empty".to_string()));
        }
        self.connection.execute(
            "INSERT INTO task_yields(report_id, item_name, provenance, quantity, skill, xp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_yield.report_id,
                new_yield.item_name,
                new_yield.provenance,
                new_yield.quantity,
                new_yield.skill,
                new_yield.xp,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Every yield line of one report.
    pub fn yields_for_report(&self, report_id: i64) -> Result<Vec<TaskYield>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT yield_id, report_id, item_name, provenance, quantity, skill, xp \
             FROM task_yields WHERE report_id = ?1 ORDER BY yield_id",
        )?;
        let mapped = statement.query_map(rusqlite::params![report_id], map_yield)?;
        let mut lines = Vec::new();
        for mapped_row in mapped {
            lines.push(mapped_row?);
        }
        Ok(lines)
    }

    /// Recomputes summed quantities per (method, item, provenance) join.
    /// Reports are counted distinct so multi-line reports count once.
    pub fn aggregate_yields(&self) -> Result<Vec<YieldAggregate>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT reports.method, yields.item_name, yields.provenance, \
             SUM(yields.quantity), SUM(yields.xp), COUNT(DISTINCT yields.report_id) \
             FROM task_yields AS yields \
             JOIN task_reports AS reports ON reports.report_id = yields.report_id \
             GROUP BY reports.method, yields.item_name, yields.provenance \
             ORDER BY reports.method, yields.item_name",
        )?;
        let mapped = statement.query_map([], |row| {
            Ok(YieldAggregate {
                method: row.get(0)?,
                item_name: row.get(1)?,
                provenance: row.get(2)?,
                total_quantity: row.get(3)?,
                total_xp: row.get(4)?,
                report_count: row.get(5)?,
            })
        })?;
        let mut aggregates = Vec::new();
        for mapped_row in mapped {
            aggregates.push(mapped_row?);
        }
        Ok(aggregates)
    }
}

#[cfg(test)]
mod reports_tests {
    use super::*;

    #[test]
    fn report_and_aggregate_roundtrip() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let report_id = store.record_task_report(&NewTaskReport {
            account: "t01".to_string(),
            node_id: "n7".to_string(),
            method: "gather:fishing/shrimp_anchovy".to_string(),
            script: "fishing".to_string(),
            ticks: 6000,
            stop: "budget_exhausted".to_string(),
            context_level: 20,
            clock: 1_700_000_000,
        })?;
        store.record_task_yield(&NewTaskYield {
            report_id,
            item_name: "Raw shrimps".to_string(),
            provenance: "produced".to_string(),
            quantity: 300,
            skill: String::new(),
            xp: 0,
        })?;
        store.record_task_yield(&NewTaskYield {
            report_id,
            item_name: String::new(),
            provenance: "produced".to_string(),
            quantity: 0,
            skill: "fishing".to_string(),
            xp: 9000,
        })?;
        assert_eq!(store.yields_for_report(report_id)?.len(), 2);
        let aggregates = store.aggregate_yields()?;
        assert_eq!(aggregates.len(), 2);
        let shrimp_line = aggregates
            .iter()
            .find(|line| line.item_name == "Raw shrimps")
            .ok_or_else(|| Error::NotFound("shrimp aggregate".to_string()))?;
        assert_eq!(shrimp_line.total_quantity, 300);
        assert_eq!(shrimp_line.report_count, 1);
        Ok(())
    }
}
