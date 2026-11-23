#![forbid(unsafe_code)]
//! Aggregates behind the XP page: XP, work ticks and run counts of
//! the task reports of a window, summed in SQL per time bucket, account,
//! planner method and skill, plus the work ticks of every report per
//! bucket and account. Raw sums only; the orchestrator divides and
//! excludes accounts. A bucket width of `i64::MAX` folds the whole
//! window into bucket 0 (all-time totals).
//!
//! The window is read through the clock index
//! (`schema::REPORT_CLOCK_INDEX`), then each report's yields through the
//! report index. A report holds one yield line per skill (its ticks are
//! summed once per yield line); reports without work ticks (older
//! measure versions) and zero XP lines are left out.

use rusqlite::params;

use crate::{Error, Store};

/// Sums of one (bucket, account, planner method, skill).
#[derive(Clone, Debug, PartialEq)]
pub struct SkillBucket {
    pub bucket_start: i64,
    pub account: String,
    pub planner_method: String,
    pub skill: String,
    pub xp: i64,
    /// Work ticks of the reports that gave XP in the skill.
    pub work_ticks: i64,
    /// Number of those reports.
    pub runs: i64,
}

/// Work ticks of one bucket and account (every report).
#[derive(Clone, Debug, PartialEq)]
pub struct BucketWork {
    pub bucket_start: i64,
    pub account: String,
    pub work_ticks: i64,
}

const SKILL_XP_SQL: &str = "SELECT (r.created_clock / ?2) * ?2, r.account, r.planner_method, y.skill, SUM(y.xp), SUM(r.work_ticks), COUNT(*) \
     FROM task_reports r CROSS JOIN task_yields y ON y.report_id = r.report_id \
     WHERE r.created_clock >= ?1 AND r.work_ticks > 0 AND y.skill <> '' AND y.xp > 0 \
     GROUP BY 1, r.account, r.planner_method, y.skill";

const BUCKET_WORK_SQL: &str = "SELECT (created_clock / ?2) * ?2, account, SUM(work_ticks) FROM task_reports \
     WHERE created_clock >= ?1 AND work_ticks > 0 GROUP BY 1, account";

impl Store {
    /// XP sums of the reports created at or after `since`, per bucket.
    pub fn skill_buckets_since(&self, since: i64, bucket_seconds: i64) -> Result<Vec<SkillBucket>, Error> {
        let mut statement = self.connection.prepare_cached(SKILL_XP_SQL)?;
        let rows = statement.query_map(params![since, bucket_seconds.max(1)], |row| {
            Ok(SkillBucket {
                bucket_start: row.get(0)?,
                account: row.get(1)?,
                planner_method: row.get(2)?,
                skill: row.get(3)?,
                xp: row.get(4)?,
                work_ticks: row.get(5)?,
                runs: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Work ticks per bucket and account since `since`.
    pub fn bucket_work_since(&self, since: i64, bucket_seconds: i64) -> Result<Vec<BucketWork>, Error> {
        let mut statement = self.connection.prepare_cached(BUCKET_WORK_SQL)?;
        let rows = statement.query_map(params![since, bucket_seconds.max(1)], |row| {
            Ok(BucketWork { bucket_start: row.get(0)?, account: row.get(1)?, work_ticks: row.get(2)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Clock of the oldest task report with work ticks, None when empty.
    pub fn oldest_work_clock(&self) -> Result<Option<i64>, Error> {
        Ok(self.connection.query_row("SELECT MIN(created_clock) FROM task_reports WHERE work_ticks > 0", [], |row| row.get::<_, Option<i64>>(0))?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Error, NewTaskReport, NewTaskYield, ReportMeasure, Store};

    fn report(store: &Store, account: &str, clock: i64, work_ticks: i64, yields: &[(&str, i64)]) -> Result<(), Error> {
        let report_id = store.record_task_report(&NewTaskReport {
            account: account.to_string(),
            node_id: "n".to_string(),
            method: "m".to_string(),
            script: "s".to_string(),
            ticks: work_ticks,
            stop: "goal_reached".to_string(),
            context_level: 1,
            clock,
        })?;
        store.record_report_measure(report_id, &ReportMeasure { work_ticks, planner_method: "gather:x".to_string(), ..ReportMeasure::default() })?;
        for (skill, xp) in yields {
            store.record_task_yield(&NewTaskYield { report_id, item_name: String::new(), provenance: "xp".to_string(), quantity: 0, skill: skill.to_string(), xp: *xp })?;
        }
        Ok(())
    }

    #[test]
    fn buckets_sum_xp_ticks_and_runs_and_skip_old_or_idle_reports() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        report(&store, "ann", 100, 600, &[("attack", 30), ("hitpoints", 10)])?;
        report(&store, "ann", 200, 400, &[("attack", 20)])?;
        report(&store, "ann", 50, 600, &[("attack", 99)])?;
        report(&store, "bob", 200, 0, &[("attack", 99)])?;
        report(&store, "bob", 3700, 400, &[("attack", 0)])?;
        let mut rows = store.skill_buckets_since(100, 3600)?;
        rows.sort_by(|left, right| left.skill.cmp(&right.skill));
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].skill.as_str(), rows[0].xp, rows[0].work_ticks, rows[0].runs), ("attack", 50, 1000, 2));
        assert_eq!((rows[1].skill.as_str(), rows[1].xp, rows[1].runs), ("hitpoints", 10, 1));
        let whole = store.skill_buckets_since(0, i64::MAX)?;
        assert!(whole.iter().all(|row| row.bucket_start == 0));
        assert_eq!(whole.iter().find(|row| row.skill == "attack").map(|row| row.xp), Some(149));
        let mut buckets = store.bucket_work_since(100, 3600)?;
        buckets.sort_by_key(|bucket| bucket.bucket_start);
        assert_eq!(buckets.len(), 2);
        assert_eq!((buckets[0].bucket_start, buckets[0].work_ticks), (0, 1000));
        assert_eq!(store.oldest_work_clock()?, Some(50));
        Ok(())
    }
}
