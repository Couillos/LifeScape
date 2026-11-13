#![forbid(unsafe_code)]
//! Item holdings history (PRD 2, D48): hourly points kept 8 days, daily
//! points (last measure of each day) kept 400 days.
//!
//! The sampler records one point per item per hour; the daily row of an
//! (item, day) only moves forward in time, so replays and imports keep
//! the last measure of the day. Retention is enforced on every record.

use crate::{Error, Store};

/// Seconds of hourly history kept: 8 days.
pub const HOURLY_KEEP_SECONDS: i64 = 8 * 86_400;
/// Seconds of daily history kept: 400 days.
pub const DAILY_KEEP_SECONDS: i64 = 400 * 86_400;
/// One holdings point per hour.
pub const HOUR_SECONDS: i64 = 3_600;
/// One daily bucket per day.
pub const DAY_SECONDS: i64 = 86_400;

impl Store {
    /// Records one holdings snapshot: `(item_id, count)` pairs at
    /// `taken_at` clock seconds. The hourly point lands on its hour
    /// (last write of the hour wins); the daily row keeps the latest
    /// measure of its day. Old rows beyond retention are deleted.
    pub fn record_holdings(&self, taken_at: i64, items: &[(i64, i64)]) -> Result<(), Error> {
        if taken_at <= 0 {
            return Err(Error::Invalid(format!("bad taken_at {taken_at}")));
        }
        let hour = (taken_at / HOUR_SECONDS) * HOUR_SECONDS;
        let day = (taken_at / DAY_SECONDS) * DAY_SECONDS;
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let recorded = self.record_inner(hour, day, taken_at, items);
        match recorded {
            Ok(()) => self.connection.execute_batch("COMMIT")?,
            Err(error) => {
                let _ignored = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        self.prune_holdings(taken_at)?;
        Ok(())
    }

    fn record_inner(&self, hour: i64, day: i64, taken_at: i64, items: &[(i64, i64)]) -> Result<(), Error> {
        for (item_id, count) in items {
            if *item_id <= 0 || *count <= 0 {
                return Err(Error::Invalid(format!("bad holdings point ({item_id}, {count})")));
            }
            self.connection.execute(
                "INSERT INTO holdings_hourly(item_id, taken_at, count) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(item_id, taken_at) DO UPDATE SET count = excluded.count",
                rusqlite::params![item_id, hour, count],
            )?;
            self.connection.execute(
                "INSERT INTO holdings_daily(item_id, day_start, taken_at, count) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(item_id, day_start) DO UPDATE SET taken_at = excluded.taken_at, count = excluded.count \
                 WHERE excluded.taken_at >= holdings_daily.taken_at",
                rusqlite::params![item_id, day, taken_at, count],
            )?;
        }
        Ok(())
    }

    /// Deletes hourly points older than 8 days and daily rows older
    /// than 400 days, relative to `now` clock seconds.
    pub fn prune_holdings(&self, now: i64) -> Result<(), Error> {
        self.connection.execute(
            "DELETE FROM holdings_hourly WHERE taken_at < ?1",
            rusqlite::params![now - HOURLY_KEEP_SECONDS],
        )?;
        self.connection.execute(
            "DELETE FROM holdings_daily WHERE day_start < ?1",
            rusqlite::params![now - DAILY_KEEP_SECONDS],
        )?;
        Ok(())
    }

    /// Latest hourly point time, `None` on an empty table (the import
    /// trigger: the JSON history is only imported into an empty store).
    pub fn holdings_latest(&self) -> Result<Option<i64>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT MAX(taken_at) FROM holdings_hourly")?;
        let found: Option<Option<i64>> = statement
            .query_map([], |row| row.get(0))?
            .next()
            .transpose()?;
        Ok(found.flatten())
    }

    /// Every item count of the last hourly point taken at or before
    /// `clock` (`(taken_at, [(item, count)])`), `None` when no point is
    /// that old: the 24 h baseline of the item holdings list.
    pub fn holdings_at_or_before(&self, clock: i64) -> Result<Option<(i64, Vec<(i64, i64)>)>, Error> {
        let mut latest = self.connection.prepare("SELECT MAX(taken_at) FROM holdings_hourly WHERE taken_at <= ?1")?;
        let taken: Option<i64> = latest.query_map(rusqlite::params![clock], |row| row.get(0))?.next().transpose()?.flatten();
        let Some(taken_at) = taken else { return Ok(None) };
        let mut rows = self.connection.prepare("SELECT item_id, count FROM holdings_hourly WHERE taken_at = ?1 ORDER BY item_id")?;
        let mut counts = Vec::new();
        for mapped in rows.query_map(rusqlite::params![taken_at], |row| Ok((row.get(0)?, row.get(1)?)))? {
            counts.push(mapped?);
        }
        Ok(Some((taken_at, counts)))
    }

    fn holdings_points_in(
        &self,
        table: &str,
        time_column: &str,
        item_id: Option<i64>,
    ) -> Result<Vec<(i64, i64)>, Error> {
        fn map_point(row: &rusqlite::Row<'_>) -> Result<(i64, i64), rusqlite::Error> {
            Ok((row.get(0)?, row.get(1)?))
        }
        let sql = match item_id {
            Some(_) => format!(
                "SELECT {time_column}, count FROM {table} WHERE item_id = ?1 ORDER BY {time_column}"
            ),
            None => format!(
                "SELECT {time_column}, SUM(count) FROM {table} GROUP BY {time_column} ORDER BY {time_column}"
            ),
        };
        let mut statement = self.connection.prepare(&sql)?;
        let mut points = Vec::new();
        match item_id {
            Some(item) => {
                for mapped_row in statement.query_map(rusqlite::params![item], map_point)? {
                    points.push(mapped_row?);
                }
            }
            None => {
                for mapped_row in statement.query_map([], map_point)? {
                    points.push(mapped_row?);
                }
            }
        }
        Ok(points)
    }

    /// Hourly curve, oldest first: one item (`Some`) or the whole
    /// economy summed per hour (`None`).
    pub fn hourly_points(&self, item_id: Option<i64>) -> Result<Vec<(i64, i64)>, Error> {
        self.holdings_points_in("holdings_hourly", "taken_at", item_id)
    }

    /// Daily curve, oldest first: day buckets with the last measure
    /// of each day, one item (`Some`) or the economy total (`None`).
    pub fn daily_points(&self, item_id: Option<i64>) -> Result<Vec<(i64, i64)>, Error> {
        self.holdings_points_in("holdings_daily", "day_start", item_id)
    }
}

#[cfg(test)]
mod holdings_tests {
    use super::*;

    #[test]
    fn daily_keeps_the_last_measure_of_the_day() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let morning = 1_762_675_200; // an exact hour inside one day
        // Same item, same day: the evening measure wins, one row kept.
        store.record_holdings(morning, &[(995, 7)])?;
        store.record_holdings(morning + 5 * HOUR_SECONDS, &[(995, 11)])?;
        assert_eq!(store.daily_points(Some(995))?.len(), 1);
        assert_eq!(store.daily_points(Some(995))?, vec![((morning / DAY_SECONDS) * DAY_SECONDS, 11)]);
        // An older replay of the same day never moves the row backwards.
        store.record_holdings(morning + HOUR_SECONDS, &[(995, 3)])?;
        assert_eq!(store.daily_points(Some(995))?.last(), Some(&((morning / DAY_SECONDS) * DAY_SECONDS, 11)));
        // Hourly keeps every hour of the day.
        assert_eq!(store.hourly_points(Some(995))?.len(), 3);
        Ok(())
    }

    #[test]
    fn baseline_is_the_last_point_at_or_before() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let hour = 1_762_675_200;
        assert_eq!(store.holdings_at_or_before(hour)?, None, "no point that old");
        store.record_holdings(hour, &[(995, 7), (1511, 3)])?;
        store.record_holdings(hour + HOUR_SECONDS, &[(995, 9)])?;
        assert_eq!(store.holdings_at_or_before(hour + HOUR_SECONDS - 1)?, Some((hour, vec![(995, 7), (1511, 3)])));
        assert_eq!(store.holdings_at_or_before(hour + 2 * HOUR_SECONDS)?.map(|(taken, _)| taken), Some(hour + HOUR_SECONDS));
        Ok(())
    }

    #[test]
    fn retention_drops_old_points() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let now = 1_762_675_200;
        store.record_holdings(now - HOURLY_KEEP_SECONDS - HOUR_SECONDS, &[(995, 1)])?;
        store.record_holdings(now - DAILY_KEEP_SECONDS - DAY_SECONDS, &[(995, 2)])?;
        assert_eq!(store.hourly_points(Some(995))?.len(), 2, "pre-prune state");
        store.record_holdings(now, &[(995, 9)])?;
        assert_eq!(store.hourly_points(Some(995))?, vec![(now, 9)], "hourly older than 8 days is gone");
        assert_eq!(store.holdings_latest()?, Some(now));
        Ok(())
    }

    #[test]
    fn total_sums_every_item_per_slot() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let hour = 1_762_675_200;
        store.record_holdings(hour, &[(995, 7), (1521, 3)])?;
        assert_eq!(store.hourly_points(None)?, vec![(hour, 10)]);
        assert_eq!(store.daily_points(None)?.len(), 1);
        assert_eq!(store.holdings_latest()?, Some(hour));
        Ok(())
    }

    #[test]
    fn bad_points_are_rejected() {
        let store = Store::open_in_memory().expect("memory store opens");
        assert!(store.record_holdings(0, &[(995, 1)]).is_err());
        assert!(store.record_holdings(100, &[(0, 1)]).is_err());
        assert!(store.record_holdings(100, &[(995, 0)]).is_err());
        assert_eq!(store.holdings_latest().expect("latest reads"), None);
    }
}
