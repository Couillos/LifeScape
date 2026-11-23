#![forbid(unsafe_code)]
//! Migration 13 (2026-11-23, store compaction) and the lossless delta
//! encoding of `task_levels`.
//!
//! Before: every report stored all of its account's skill levels
//! (4.6 M rows, 0.8 % of them different from the account's previous
//! report). Now a report stores a level row only when the value differs
//! from the latest stored one of that account and skill, or when the skill
//! is new for the account; `task_reports.levels_mode` says how to read it:
//!
//! * mode 0 (default): the rows with this `report_id` ARE the report's
//!   levels (full set; also the encoding of a report with no level at all
//!   and of any report whose skill set does not extend the account's known
//!   one: a subset of skills is never guessed from older rows);
//! * mode 1: the report carried every skill the account had stored before
//!   it (plus possibly new ones); its levels are, per skill, the row of the
//!   account with the greatest `report_id` not above this report's.
//!
//! The reconstruction is exact for every report of the old format
//! (`levels_tests`, and the real-data comparison `real_data_levels_check`).

use crate::{Error, Store};
use std::collections::HashMap;

/// Steps of migration 13 that are plain SQL: indexes, the mode column and
/// the tables rebuilt WITHOUT ROWID (their autoindex duplicated the table).
pub(crate) const MIGRATION_V13_SCHEMA: &str = "
DROP INDEX IF EXISTS idx_task_yields_item;
DROP INDEX IF EXISTS idx_task_flows_item;
DROP INDEX IF EXISTS idx_orders_replaces;
DROP INDEX IF EXISTS idx_task_reports_planner;
DROP INDEX IF EXISTS idx_task_reports_method;
DROP INDEX IF EXISTS idx_task_reports_measure;
DROP INDEX IF EXISTS idx_orders_item_status;
DROP INDEX IF EXISTS idx_orders_account;
CREATE INDEX IF NOT EXISTS idx_orders_open_book ON orders(item_id, side, price) WHERE status = 'open';
CREATE INDEX IF NOT EXISTS idx_orders_live_book ON orders(item_id, side) WHERE status IN ('open','matched');
CREATE INDEX IF NOT EXISTS idx_orders_live_account ON orders(account) WHERE status IN ('open','matched');
CREATE INDEX IF NOT EXISTS idx_orders_live_clock ON orders(created_clock) WHERE status IN ('open','matched');
CREATE INDEX IF NOT EXISTS idx_orders_horizon ON orders(account, updated_clock) WHERE status = 'cancelled' AND cancel_reason = 'horizon';
CREATE INDEX IF NOT EXISTS idx_deals_live ON deals(created_clock) WHERE status NOT IN ('settled','failed');
ALTER TABLE task_reports ADD COLUMN levels_mode INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_levels RENAME TO task_levels_old;
CREATE TABLE task_levels(
    account TEXT NOT NULL,
    skill TEXT NOT NULL,
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    level INTEGER NOT NULL,
    PRIMARY KEY(account, skill, report_id)
) WITHOUT ROWID;
ALTER TABLE task_units RENAME TO task_units_old;
CREATE TABLE task_units(
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    unit TEXT NOT NULL,
    subject TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    PRIMARY KEY(report_id, unit, subject)
) WITHOUT ROWID;
INSERT INTO task_units(report_id, unit, subject, quantity) SELECT report_id, unit, subject, quantity FROM task_units_old;
DROP TABLE task_units_old;
";

fn sql(step: &'static str) -> impl Fn(rusqlite::Error) -> Error {
    move |source| Error::Sql(format!("migration 13 {step}: {source}"))
}

/// Migration 13, inside the migration transaction (the caller bumps the
/// version).
pub(crate) fn apply_v13(connection: &rusqlite::Connection) -> Result<(), Error> {
    connection.execute_batch(MIGRATION_V13_SCHEMA).map_err(sql("schema"))?;
    let mut read = connection
        .prepare(
            "SELECT r.report_id, r.account, l.skill, l.level FROM task_levels_old l \
             JOIN task_reports r ON r.report_id = l.report_id ORDER BY r.account, r.report_id, l.skill",
        )
        .map_err(sql("read levels"))?;
    let mut insert = connection
        .prepare("INSERT INTO task_levels(account, skill, report_id, level) VALUES (?1, ?2, ?3, ?4)")
        .map_err(sql("insert levels"))?;
    let mut mark = connection.prepare("UPDATE task_reports SET levels_mode = 1 WHERE report_id = ?1").map_err(sql("mark"))?;
    let mut rows = read.query([]).map_err(sql("read levels"))?;
    let mut account = String::new();
    let mut carried: HashMap<String, i64> = HashMap::new();
    let mut group: Vec<(String, i64)> = Vec::new();
    let mut group_id = 0_i64;
    let mut flush = |account: &str, id: i64, group: &mut Vec<(String, i64)>, carried: &mut HashMap<String, i64>| -> Result<(), Error> {
        if group.is_empty() {
            return Ok(());
        }
        let known = group.iter().filter(|(skill, _)| carried.contains_key(skill)).count();
        let delta = known == carried.len();
        for (skill, level) in group.iter() {
            if !delta || carried.get(skill) != Some(level) {
                insert.execute(rusqlite::params![account, skill, id, level]).map_err(sql("write levels"))?;
            }
            carried.insert(skill.clone(), *level);
        }
        if delta {
            mark.execute([id]).map_err(sql("mark"))?;
        }
        group.clear();
        Ok(())
    };
    while let Some(row) = rows.next().map_err(sql("read levels"))? {
        let id: i64 = row.get(0).map_err(sql("row"))?;
        let who: String = row.get(1).map_err(sql("row"))?;
        let skill: String = row.get(2).map_err(sql("row"))?;
        let level: i64 = row.get(3).map_err(sql("row"))?;
        if who != account || id != group_id {
            flush(&account, group_id, &mut group, &mut carried)?;
            if who != account {
                carried.clear();
                account = who;
            }
            group_id = id;
        }
        group.push((skill, level));
    }
    flush(&account, group_id, &mut group, &mut carried)?;
    drop(rows);
    drop(read);
    connection.execute_batch("DROP TABLE task_levels_old;").map_err(sql("drop old levels"))
}

impl Store {
    /// Stores the levels of report `report_id` (delta encoding above).
    pub(crate) fn write_levels(&self, report_id: i64, levels: &[(String, i64)]) -> Result<(), Error> {
        if levels.is_empty() {
            return Ok(());
        }
        let account: String = self.connection.query_row("SELECT account FROM task_reports WHERE report_id = ?1", [report_id], |row| row.get(0))?;
        let mut seen = std::collections::HashSet::new();
        let duplicated = !levels.iter().all(|(skill, _)| seen.insert(skill.as_str()));
        let carried = self.carried_levels(&account, report_id - 1)?;
        let known = levels.iter().filter(|(skill, _)| carried.contains_key(skill)).count();
        let later = self.later_delta_reports(&account, report_id)?;
        let delta = !duplicated && known == carried.len() && later.is_empty();
        if !later.is_empty() {
            // A report arriving after a newer one of its account: the newer
            // delta reports read this one's rows, so they are first written
            // out in full (read before our rows exist) and stay exact.
            let mut frozen = Vec::new();
            for later_id in &later {
                frozen.push((*later_id, self.levels_of(&account, *later_id, 1)?));
            }
            for (later_id, set) in frozen {
                for (skill, level) in set {
                    self.connection
                        .prepare_cached("INSERT OR REPLACE INTO task_levels(account, skill, report_id, level) VALUES (?1, ?2, ?3, ?4)")?
                        .execute(rusqlite::params![account, skill, later_id, level])?;
                }
                self.connection.prepare_cached("UPDATE task_reports SET levels_mode = 0 WHERE report_id = ?1")?.execute([later_id])?;
            }
        }
        let mut insert = self.connection.prepare_cached("INSERT INTO task_levels(account, skill, report_id, level) VALUES (?1, ?2, ?3, ?4)")?;
        for (skill, level) in levels {
            if !delta || carried.get(skill) != Some(level) {
                insert.execute(rusqlite::params![account, skill, report_id, level])?;
            }
        }
        if delta {
            self.connection.prepare_cached("UPDATE task_reports SET levels_mode = 1 WHERE report_id = ?1")?.execute([report_id])?;
        }
        Ok(())
    }

    /// Latest stored level per skill of `account` among reports `<= up_to`.
    fn carried_levels(&self, account: &str, up_to: i64) -> Result<HashMap<String, i64>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT skill, level, MAX(report_id) FROM task_levels WHERE account = ?1 AND report_id <= ?2 GROUP BY skill")?;
        let rows = statement.query_map(rusqlite::params![account, up_to], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (skill, level) = row?;
            out.insert(skill, level);
        }
        Ok(out)
    }

    /// Delta reports of `account` newer than `report_id`, oldest first.
    fn later_delta_reports(&self, account: &str, report_id: i64) -> Result<Vec<i64>, Error> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT report_id FROM task_reports WHERE report_id > ?1 AND account = ?2 AND levels_mode = 1 ORDER BY report_id")?;
        let rows = statement.query_map(rusqlite::params![report_id, account], |row| row.get::<_, i64>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The levels of one report in the old per-report form, sorted by
    /// skill. `mode` is the report's `levels_mode`.
    pub(crate) fn levels_of(&self, account: &str, report_id: i64, mode: i64) -> Result<Vec<(String, i64)>, Error> {
        let sql = if mode == 1 {
            "SELECT skill, level, MAX(report_id) FROM task_levels WHERE account = ?1 AND report_id <= ?2 GROUP BY skill ORDER BY skill"
        } else {
            "SELECT skill, level, report_id FROM task_levels WHERE account = ?1 AND report_id = ?2 ORDER BY skill"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let rows = statement.query_map(rusqlite::params![account, report_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
