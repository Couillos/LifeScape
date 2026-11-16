#![forbid(unsafe_code)]
//! Yield measures of report contract 2 (plan rendements R1, schema v5):
//! the run context columns of `task_reports`, every skill's level at the
//! start, the signed flows by cause, the XP-bearing units and the
//! alerts. And the purge of the measure-1 rows (D8): destructive, run
//! only on the user's confirmation (`orchestrator purge-measures-v1`).

use crate::{Error, Store};

/// Contract-2 measure of one stored report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReportMeasure {
    pub planner_method: String,
    pub method_source: String,
    pub site: String,
    pub target: String,
    pub tool: String,
    pub combat_style: String,
    pub work_ticks: i64,
    pub pause_ticks: i64,
    /// (skill, level at the start).
    pub levels: Vec<(String, i64)>,
    /// (item, cause, signed quantity).
    pub flows: Vec<(String, String, i64)>,
    /// (unit kind, subject, quantity).
    pub units: Vec<(String, String, i64)>,
    /// (alert kind, item, quantity).
    pub alerts: Vec<(String, String, i64)>,
}

/// Rows a v1 purge removes (or would remove).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PurgeCounts {
    pub reports: i64,
    pub yields: i64,
}

/// One stored contract-2 report read back in order (the rates rebuild).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMeasure {
    pub report_id: i64,
    pub account: String,
    pub stop: String,
    pub clock: i64,
    pub measure: ReportMeasure,
}

/// Measure version of a contract-2 report.
pub const MEASURE_VERSION: i64 = 2;

impl Store {
    /// Marks report `report_id` measure 2 and stores its contract-2 rows,
    /// all or nothing.
    pub fn record_report_measure(&self, report_id: i64, measure: &ReportMeasure) -> Result<(), Error> {
        if measure.work_ticks < 0 || measure.pause_ticks < 0 {
            return Err(Error::Invalid(format!("bad measure ticks {} / {}", measure.work_ticks, measure.pause_ticks)));
        }
        let owned = self.begin_batch()?;
        let written = self.write_measure(report_id, measure);
        if owned {
            match &written {
                Ok(()) => self.commit_batch()?,
                Err(_) => {
                    let _ignored = self.connection.execute_batch("ROLLBACK");
                }
            }
        }
        written
    }

    fn write_measure(&self, report_id: i64, measure: &ReportMeasure) -> Result<(), Error> {
        let updated = self.connection.execute(
            "UPDATE task_reports SET measure_version = ?2, planner_method = ?3, method_source = ?4, site = ?5, target = ?6, \
             tool = ?7, combat_style = ?8, work_ticks = ?9, pause_ticks = ?10 WHERE report_id = ?1",
            rusqlite::params![
                report_id,
                MEASURE_VERSION,
                measure.planner_method,
                measure.method_source,
                measure.site,
                measure.target,
                measure.tool,
                measure.combat_style,
                measure.work_ticks,
                measure.pause_ticks,
            ],
        )?;
        if updated != 1 {
            return Err(Error::NotFound(format!("task report {report_id}")));
        }
        for (skill, level) in &measure.levels {
            self.connection.execute("INSERT INTO task_levels(report_id, skill, level) VALUES (?1, ?2, ?3)", rusqlite::params![report_id, skill, level])?;
        }
        for (item, cause, quantity) in &measure.flows {
            self.connection.execute(
                "INSERT INTO task_flows(report_id, item_name, cause, quantity) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![report_id, item, cause, quantity],
            )?;
        }
        for (unit, subject, quantity) in &measure.units {
            self.connection.execute(
                "INSERT INTO task_units(report_id, unit, subject, quantity) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![report_id, unit, subject, quantity],
            )?;
        }
        for (kind, item, quantity) in &measure.alerts {
            self.connection.execute(
                "INSERT INTO task_alerts(report_id, kind, item_name, quantity) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![report_id, kind, item, quantity],
            )?;
        }
        Ok(())
    }

    /// The contract-2 measure of one report, `None` for a measure-1 row.
    pub fn report_measure(&self, report_id: i64) -> Result<Option<ReportMeasure>, Error> {
        let head = self.connection.query_row(
            "SELECT measure_version, planner_method, method_source, site, target, tool, combat_style, work_ticks, pause_ticks \
             FROM task_reports WHERE report_id = ?1",
            rusqlite::params![report_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    ReportMeasure {
                        planner_method: row.get(1)?,
                        method_source: row.get(2)?,
                        site: row.get(3)?,
                        target: row.get(4)?,
                        tool: row.get(5)?,
                        combat_style: row.get(6)?,
                        work_ticks: row.get(7)?,
                        pause_ticks: row.get(8)?,
                        ..ReportMeasure::default()
                    },
                ))
            },
        );
        let (version, mut measure) = match head {
            Ok(found) => found,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Err(Error::NotFound(format!("task report {report_id}"))),
            Err(other) => return Err(other.into()),
        };
        if version < MEASURE_VERSION {
            return Ok(None);
        }
        measure.levels = self.pairs("SELECT skill, level FROM task_levels WHERE report_id = ?1 ORDER BY skill", report_id)?;
        measure.flows = self.triples("SELECT item_name, cause, quantity FROM task_flows WHERE report_id = ?1 ORDER BY flow_id", report_id)?;
        measure.units = self.triples("SELECT unit, subject, quantity FROM task_units WHERE report_id = ?1 ORDER BY unit, subject", report_id)?;
        measure.alerts = self.triples("SELECT kind, item_name, quantity FROM task_alerts WHERE report_id = ?1 ORDER BY alert_id", report_id)?;
        Ok(Some(measure))
    }

    fn pairs(&self, sql: &str, report_id: i64) -> Result<Vec<(String, i64)>, Error> {
        let mut statement = self.connection.prepare(sql)?;
        let rows = statement.query_map(rusqlite::params![report_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn triples(&self, sql: &str, report_id: i64) -> Result<Vec<(String, String, i64)>, Error> {
        let mut statement = self.connection.prepare(sql)?;
        let rows = statement.query_map(rusqlite::params![report_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Contract-2 reports after `after_id`, oldest first, at most
    /// `limit` (the rates snapshot rebuilds from them page by page).
    pub fn measures_after(&self, after_id: i64, limit: i64) -> Result<Vec<StoredMeasure>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT report_id, account, stop, created_clock FROM task_reports WHERE measure_version >= ?1 AND report_id > ?2 ORDER BY report_id LIMIT ?3",
        )?;
        let rows = statement.query_map(rusqlite::params![MEASURE_VERSION, after_id, limit], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?)))?;
        let mut heads = Vec::new();
        for row in rows {
            heads.push(row?);
        }
        let mut out = Vec::with_capacity(heads.len());
        for (report_id, account, stop, clock) in heads {
            if let Some(measure) = self.report_measure(report_id)? {
                out.push(StoredMeasure { report_id, account, stop, clock, measure });
            }
        }
        Ok(out)
    }

    /// Rows a v1 purge would remove: measure-1 reports and their lines.
    pub fn v1_measure_counts(&self) -> Result<PurgeCounts, Error> {
        let reports = self.connection.query_row("SELECT COUNT(*) FROM task_reports WHERE measure_version < ?1", rusqlite::params![MEASURE_VERSION], |row| row.get(0))?;
        let yields = self.connection.query_row(
            "SELECT COUNT(*) FROM task_yields WHERE report_id IN (SELECT report_id FROM task_reports WHERE measure_version < ?1)",
            rusqlite::params![MEASURE_VERSION],
            |row| row.get(0),
        )?;
        Ok(PurgeCounts { reports, yields })
    }

    /// Deletes every measure-1 report and its yield lines (D8), in one
    /// transaction. Destructive: callers run it on the user's explicit
    /// confirmation only. Measure-2 rows are never touched.
    pub fn purge_v1_measures(&self) -> Result<PurgeCounts, Error> {
        let counts = self.v1_measure_counts()?;
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let purged = self
            .connection
            .execute(
                "DELETE FROM task_yields WHERE report_id IN (SELECT report_id FROM task_reports WHERE measure_version < ?1)",
                rusqlite::params![MEASURE_VERSION],
            )
            .and_then(|_| self.connection.execute("DELETE FROM task_reports WHERE measure_version < ?1", rusqlite::params![MEASURE_VERSION]));
        match purged {
            Ok(_) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(counts)
            }
            Err(source) => {
                let _ignored = self.connection.execute_batch("ROLLBACK");
                Err(Error::Sql(format!("purge v1 measures: {source}")))
            }
        }
    }
}

#[cfg(test)]
mod measures_tests {
    use super::{PurgeCounts, ReportMeasure};
    use crate::{Error, NewTaskReport, NewTaskYield, Store};

    fn report(store: &Store, method: &str) -> Result<i64, Error> {
        let report_id = store.record_task_report(&NewTaskReport {
            account: "t01".to_string(),
            node_id: "n1".to_string(),
            method: method.to_string(),
            script: method.to_string(),
            ticks: 600,
            stop: "goal_reached".to_string(),
            context_level: 5,
            clock: 1,
        })?;
        store.record_task_yield(&NewTaskYield { report_id, item_name: "Logs".to_string(), provenance: "produced".to_string(), quantity: 9, skill: String::new(), xp: 0 })?;
        Ok(report_id)
    }

    /// A contract-2 measure round-trips; a v1 report reads no measure.
    #[test]
    fn measure_round_trips() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let old = report(&store, "woodcut")?;
        let new = report(&store, "woodcut")?;
        let measure = ReportMeasure {
            planner_method: "gather:woodcut/logs".to_string(),
            method_source: "node".to_string(),
            site: "3190,3240,0".to_string(),
            target: "logs".to_string(),
            tool: "rune_axe".to_string(),
            combat_style: String::new(),
            work_ticks: 540,
            pause_ticks: 60,
            levels: vec![("attack".to_string(), 1), ("woodcutting".to_string(), 20)],
            flows: vec![("Logs".to_string(), "product".to_string(), 9)],
            units: vec![("product".to_string(), "Logs".to_string(), 9)],
            alerts: vec![("residual".to_string(), "Coins".to_string(), -3)],
        };
        store.record_report_measure(new, &measure)?;
        assert_eq!(store.report_measure(new)?, Some(measure.clone()));
        assert_eq!(store.report_measure(old)?, None);
        assert!(store.record_report_measure(999, &ReportMeasure::default()).is_err(), "unknown report fails loud");
        let stored = store.measures_after(0, 10)?;
        assert_eq!(stored.len(), 1, "measure-1 rows are never read back");
        assert_eq!((stored[0].report_id, stored[0].stop.as_str(), &stored[0].measure), (new, "goal_reached", &measure));
        assert!(store.measures_after(new, 10)?.is_empty());
        Ok(())
    }

    /// The purge removes measure-1 rows only, with their lines.
    #[test]
    fn purge_keeps_measure_two() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let old = report(&store, "fishing")?;
        let new = report(&store, "fishing")?;
        store.record_report_measure(new, &ReportMeasure { method_source: "none".to_string(), ..ReportMeasure::default() })?;
        assert_eq!(store.v1_measure_counts()?, PurgeCounts { reports: 1, yields: 1 });
        assert_eq!(store.purge_v1_measures()?, PurgeCounts { reports: 1, yields: 1 });
        assert!(store.yields_for_report(old)?.is_empty());
        assert_eq!(store.yields_for_report(new)?.len(), 1);
        assert_eq!(store.v1_measure_counts()?, PurgeCounts::default());
        Ok(())
    }
}
