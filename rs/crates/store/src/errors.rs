#![forbid(unsafe_code)]
//! Centralised error log (migration 3): one row per failure seen by the
//! daemon, whatever its source — task (script error / failed or blocked
//! report), nav (failure dossier), deal (market deal failure), brain
//! (abandon, stuck graph, ban). Rows carry a normalised `signature` so
//! similar errors group together, plus who/where/what for drill-down.
//!
//! Pulled sources (dossiers, deals) carry a `source_ref` unique key so a
//! rescan never duplicates a row; pushed rows leave it NULL.

use crate::{Error, Store};

/// One error row. `error_id` is 0 for a row not stored yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErrorRecord {
    pub error_id: i64,
    /// Unix clock seconds (UTC) the error was seen.
    pub clock: i64,
    /// `task` | `nav` | `deal` | `brain`.
    pub source: String,
    /// Error type inside the source (`Sdk`, `Failed`, `NoRoute`, `timeout`…).
    pub kind: String,
    pub account: String,
    /// Other party (deal seller), empty otherwise.
    pub peer: String,
    pub script: String,
    pub args: String,
    pub node_id: String,
    pub method: String,
    /// Task stop (`failed`, `blocked`) once a report is attached.
    pub stop: String,
    pub signature: String,
    pub message: String,
    /// (x, z, level) when known.
    pub position: Option<(i64, i64, i64)>,
    pub zone: String,
    /// Free-form context (nav destination, deal item and price…).
    pub detail: String,
    /// Dossier file name under `run/failures/`, empty when none.
    pub dossier: String,
    pub source_ref: Option<String>,
}

/// Row filter: clock window plus optional exact matches (empty = any).
/// `account` matches the account or the peer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErrorFilter {
    pub since: i64,
    /// Exclusive upper bound, 0 = none.
    pub until: i64,
    pub source: String,
    pub kind: String,
    pub account: String,
    pub script: String,
    pub zone: String,
    pub signature: String,
    /// 0 = no limit.
    pub limit: i64,
}

/// A report outcome folded onto the script error row the same failure
/// already produced (one failure = one row).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskOutcome {
    pub account: String,
    pub node_id: String,
    pub method: String,
    pub stop: String,
    pub kind: String,
    pub message: String,
    /// Only rows seen at or after this clock are candidates.
    pub since: i64,
}

/// A failed market deal joined with its item name.
#[derive(Clone, Debug, PartialEq)]
pub struct FailedDeal {
    pub deal_id: i64,
    pub buyer: String,
    pub seller: String,
    pub item_id: i64,
    pub item_name: String,
    pub quantity: i64,
    pub price: i64,
    pub meeting: (i64, i64, i64),
    pub reason: String,
    pub clock: i64,
}

const ERROR_COLUMNS: &str = "error_id, clock, source, kind, account, peer, script, args, node_id, \
     method, stop, signature, message, has_pos, pos_x, pos_z, level, zone, detail, dossier, source_ref";
/// Same shape without the long text columns (aggregation reads).
const SLIM_COLUMNS: &str = "error_id, clock, source, kind, account, peer, script, '', node_id, \
     method, stop, signature, '', has_pos, pos_x, pos_z, level, zone, '', dossier, source_ref";

fn map_error(row: &rusqlite::Row<'_>) -> Result<ErrorRecord, rusqlite::Error> {
    let has_pos: i64 = row.get(13)?;
    Ok(ErrorRecord {
        error_id: row.get(0)?,
        clock: row.get(1)?,
        source: row.get(2)?,
        kind: row.get(3)?,
        account: row.get(4)?,
        peer: row.get(5)?,
        script: row.get(6)?,
        args: row.get(7)?,
        node_id: row.get(8)?,
        method: row.get(9)?,
        stop: row.get(10)?,
        signature: row.get(11)?,
        message: row.get(12)?,
        position: if has_pos != 0 { Some((row.get(14)?, row.get(15)?, row.get(16)?)) } else { None },
        zone: row.get(17)?,
        detail: row.get(18)?,
        dossier: row.get(19)?,
        source_ref: row.get(20)?,
    })
}

/// WHERE clause and its bound values for a filter.
fn filter_clause(filter: &ErrorFilter) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value as SqlValue;
    let mut clauses = vec!["clock >= ?1".to_string()];
    let mut values = vec![SqlValue::Integer(filter.since)];
    if filter.until > 0 {
        values.push(SqlValue::Integer(filter.until));
        clauses.push(format!("clock < ?{}", values.len()));
    }
    for (column, wanted) in [
        ("source", &filter.source),
        ("kind", &filter.kind),
        ("script", &filter.script),
        ("zone", &filter.zone),
        ("signature", &filter.signature),
    ] {
        if !wanted.is_empty() {
            values.push(SqlValue::Text(wanted.clone()));
            clauses.push(format!("{column} = ?{}", values.len()));
        }
    }
    if !filter.account.is_empty() {
        values.push(SqlValue::Text(filter.account.clone()));
        clauses.push(format!("(account = ?{0} OR peer = ?{0})", values.len()));
    }
    (clauses.join(" AND "), values)
}

impl Store {
    /// Inserts rows in one transaction; a row whose `source_ref` already
    /// exists is skipped. Returns the number of rows inserted.
    pub fn record_errors(&self, rows: &[ErrorRecord]) -> Result<usize, Error> {
        let transaction = self.connection.unchecked_transaction()?;
        let mut inserted = 0;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO error_events(clock, source, kind, account, peer, script, args, \
                 node_id, method, stop, signature, message, has_pos, pos_x, pos_z, level, zone, detail, \
                 dossier, source_ref) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            )?;
            for row in rows {
                if row.source.is_empty() || row.kind.is_empty() {
                    return Err(Error::Invalid("error row source/kind is empty".to_string()));
                }
                let (pos_x, pos_z, level) = row.position.unwrap_or((0, 0, 0));
                inserted += statement.execute(rusqlite::params![
                    row.clock,
                    row.source,
                    row.kind,
                    row.account,
                    row.peer,
                    row.script,
                    row.args,
                    row.node_id,
                    row.method,
                    row.stop,
                    row.signature,
                    row.message,
                    i64::from(row.position.is_some()),
                    pos_x,
                    pos_z,
                    level,
                    row.zone,
                    row.detail,
                    row.dossier,
                    row.source_ref,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(inserted)
    }

    /// Folds a task outcome onto the latest matching `task` row of the
    /// account (same message, no report folded yet, recent). False when none
    /// matched: the caller then records its own row.
    pub fn attach_task_outcome(&self, outcome: &TaskOutcome) -> Result<bool, Error> {
        let changed = self.connection.execute(
            "UPDATE error_events SET node_id = ?1, method = ?2, stop = ?3, kind = ?4 \
             WHERE error_id = (SELECT error_id FROM error_events WHERE source = 'task' \
             AND account = ?5 AND message = ?6 AND stop = '' AND clock >= ?7 \
             ORDER BY error_id DESC LIMIT 1)",
            rusqlite::params![
                outcome.node_id,
                outcome.method,
                outcome.stop,
                outcome.kind,
                outcome.account,
                outcome.message,
                outcome.since,
            ],
        )?;
        Ok(changed > 0)
    }

    /// Rows matching the filter, newest first.
    pub fn errors_matching(&self, filter: &ErrorFilter) -> Result<Vec<ErrorRecord>, Error> {
        self.select_errors(ERROR_COLUMNS, filter)
    }

    /// Rows matching the filter, newest first, without `args`, `message`
    /// and `detail` (left empty): aggregation over large windows.
    pub fn errors_slim(&self, filter: &ErrorFilter) -> Result<Vec<ErrorRecord>, Error> {
        self.select_errors(SLIM_COLUMNS, filter)
    }

    fn select_errors(&self, columns: &str, filter: &ErrorFilter) -> Result<Vec<ErrorRecord>, Error> {
        let (clause, mut values) = filter_clause(filter);
        let mut sql = format!("SELECT {columns} FROM error_events WHERE {clause} ORDER BY clock DESC, error_id DESC");
        if filter.limit > 0 {
            values.push(rusqlite::types::Value::Integer(filter.limit));
            sql.push_str(&format!(" LIMIT ?{}", values.len()));
        }
        let mut statement = self.connection.prepare(&sql)?;
        let mapped = statement.query_map(rusqlite::params_from_iter(values.iter()), map_error)?;
        let mut rows = Vec::new();
        for mapped_row in mapped {
            rows.push(mapped_row?);
        }
        Ok(rows)
    }

    /// Number of rows matching the filter (limit ignored).
    pub fn count_errors(&self, filter: &ErrorFilter) -> Result<i64, Error> {
        let (clause, values) = filter_clause(filter);
        let count = self.connection.query_row(
            &format!("SELECT COUNT(*) FROM error_events WHERE {clause}"),
            rusqlite::params_from_iter(values.iter()),
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Clock of the newest row of a source, None when it has none.
    pub fn latest_error_clock(&self, source: &str) -> Result<Option<i64>, Error> {
        let found: Option<i64> = self.connection.query_row(
            "SELECT MAX(clock) FROM error_events WHERE source = ?1",
            rusqlite::params![source],
            |row| row.get(0),
        )?;
        Ok(found)
    }

    /// Drops rows older than `clock` (retention). Returns rows removed.
    pub fn purge_errors_before(&self, clock: i64) -> Result<usize, Error> {
        Ok(self
            .connection
            .execute("DELETE FROM error_events WHERE clock < ?1", rusqlite::params![clock])?)
    }

    /// Finished task reports per (script, stop) in a clock window: the
    /// run counts behind the per-script failure rate.
    pub fn task_stop_counts(&self, since: i64, until: i64) -> Result<Vec<(String, String, i64)>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT script, stop, COUNT(*) FROM task_reports WHERE created_clock >= ?1 \
             AND (?2 <= 0 OR created_clock < ?2) GROUP BY script, stop ORDER BY script, stop",
        )?;
        let mapped = statement.query_map(rusqlite::params![since, until], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut counts = Vec::new();
        for mapped_row in mapped {
            counts.push(mapped_row?);
        }
        Ok(counts)
    }

    /// (settled, failed) deals whose last move falls in the window.
    pub fn deal_outcome_counts(&self, since: i64, until: i64) -> Result<(i64, i64), Error> {
        let counts = self.connection.query_row(
            "SELECT COALESCE(SUM(status = 'settled'), 0), COALESCE(SUM(status = 'failed'), 0) FROM deals \
             WHERE updated_clock >= ?1 AND (?2 <= 0 OR updated_clock < ?2)",
            rusqlite::params![since, until],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(counts)
    }

    /// Failed deals moved at or after `since`, oldest first, with the
    /// item name (empty when the catalogue misses it).
    pub fn failed_deals_since(&self, since: i64) -> Result<Vec<FailedDeal>, Error> {
        let mut statement = self.connection.prepare(
            "SELECT deals.deal_id, deals.buyer, deals.seller, deals.item_id, COALESCE(items.name, ''), \
             deals.quantity, deals.price, deals.meeting_x, deals.meeting_y, deals.meeting_level, \
             deals.fail_reason, deals.updated_clock FROM deals LEFT JOIN items ON items.item_id = deals.item_id \
             WHERE deals.status = 'failed' AND deals.updated_clock >= ?1 ORDER BY deals.updated_clock, deals.deal_id",
        )?;
        let mapped = statement.query_map(rusqlite::params![since], |row| {
            Ok(FailedDeal {
                deal_id: row.get(0)?,
                buyer: row.get(1)?,
                seller: row.get(2)?,
                item_id: row.get(3)?,
                item_name: row.get(4)?,
                quantity: row.get(5)?,
                price: row.get(6)?,
                meeting: (row.get(7)?, row.get(8)?, row.get(9)?),
                reason: row.get(10)?,
                clock: row.get(11)?,
            })
        })?;
        let mut deals = Vec::new();
        for mapped_row in mapped {
            deals.push(mapped_row?);
        }
        Ok(deals)
    }
}

#[cfg(test)]
mod errors_tests {
    use super::*;

    fn row(clock: i64, source: &str, kind: &str, account: &str, message: &str) -> ErrorRecord {
        ErrorRecord {
            clock,
            source: source.to_string(),
            kind: kind.to_string(),
            account: account.to_string(),
            signature: message.to_string(),
            message: message.to_string(),
            ..ErrorRecord::default()
        }
    }

    #[test]
    fn record_filter_and_dedupe() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        let mut nav = row(100, "nav", "NoRoute", "t01", "no route");
        nav.source_ref = Some("nav:t01-5.json:100".to_string());
        nav.position = Some((3200, 3200, 0));
        nav.zone = "Lumbridge".to_string();
        let mut deal = row(150, "deal", "timeout", "t02", "deal timeout");
        deal.peer = "t03".to_string();
        let inserted = store.record_errors(&[nav.clone(), row(120, "task", "Sdk", "t01", "sdk: x"), deal])?;
        assert_eq!(inserted, 3);
        // Same source_ref twice: skipped, never a duplicate.
        assert_eq!(store.record_errors(&[nav])?, 0);
        let all = store.errors_matching(&ErrorFilter::default())?;
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].clock, 150, "newest first");
        let by_account = store.errors_matching(&ErrorFilter { account: "t03".to_string(), ..ErrorFilter::default() })?;
        assert_eq!(by_account.len(), 1, "peer matches the account filter");
        let windowed = ErrorFilter { since: 110, until: 150, ..ErrorFilter::default() };
        assert_eq!(store.count_errors(&windowed)?, 1);
        let zoned = store.errors_matching(&ErrorFilter { zone: "Lumbridge".to_string(), ..ErrorFilter::default() })?;
        assert_eq!(zoned[0].position, Some((3200, 3200, 0)));
        assert_eq!(store.latest_error_clock("nav")?, Some(100));
        assert_eq!(store.latest_error_clock("brain")?, None);
        let limited = store.errors_matching(&ErrorFilter { limit: 2, ..ErrorFilter::default() })?;
        assert_eq!(limited.len(), 2);
        let slim = store.errors_slim(&ErrorFilter::default())?;
        assert_eq!(slim.len(), 3);
        assert!(slim.iter().all(|row| row.message.is_empty()), "slim rows drop the text");
        assert_eq!(slim[2].signature, "no route");
        assert_eq!(store.purge_errors_before(130)?, 2);
        assert_eq!(store.count_errors(&ErrorFilter::default())?, 1);
        Ok(())
    }

    #[test]
    fn task_outcome_folds_onto_the_script_row() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        store.record_errors(&[row(100, "task", "Failed", "t01", "failed: fish failed")])?;
        let outcome = TaskOutcome {
            account: "t01".to_string(),
            node_id: "n7".to_string(),
            method: "fishing".to_string(),
            stop: "failed".to_string(),
            kind: "Failed".to_string(),
            message: "failed: fish failed".to_string(),
            since: 90,
        };
        assert!(store.attach_task_outcome(&outcome)?);
        // Already folded: a second report finds no candidate.
        assert!(!store.attach_task_outcome(&outcome)?);
        let stored = store.errors_matching(&ErrorFilter::default())?;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].node_id, "n7");
        assert_eq!(stored[0].stop, "failed");
        // Too old: no fold.
        store.record_errors(&[row(10, "task", "Failed", "t01", "failed: other")])?;
        let late = TaskOutcome { message: "failed: other".to_string(), ..outcome };
        assert!(!store.attach_task_outcome(&late)?);
        Ok(())
    }

    #[test]
    fn run_and_deal_counts() -> Result<(), Error> {
        let store = Store::open_in_memory()?;
        for (stop, clock) in [("failed", 100), ("goal_reached", 110), ("failed", 120), ("blocked", 500)] {
            store.record_task_report(&crate::NewTaskReport {
                account: "t01".to_string(),
                node_id: "n1".to_string(),
                method: "fishing".to_string(),
                script: "fishing".to_string(),
                ticks: 10,
                stop: stop.to_string(),
                context_level: 1,
                clock,
            })?;
        }
        let counts = store.task_stop_counts(0, 400)?;
        assert_eq!(
            counts,
            vec![("fishing".to_string(), "failed".to_string(), 2), ("fishing".to_string(), "goal_reached".to_string(), 1)]
        );
        crate::ensure_test_item(&store, 440)?;
        let deal_id = store.create_deal(&crate::NewDeal {
            buyer: "t01".to_string(),
            seller: "t02".to_string(),
            item_id: 440,
            quantity: 5,
            price: 10,
            meeting_x: 3213,
            meeting_y: 3429,
            meeting_level: 0,
            deadline_tick: 99,
            clock: 100,
        })?;
        store.set_deal_status(deal_id, crate::DealStatus::Failed, "no_show", 200)?;
        assert_eq!(store.deal_outcome_counts(150, 0)?, (0, 1));
        let failed = store.failed_deals_since(150)?;
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].item_name, "Test item 440");
        assert_eq!(failed[0].reason, "no_show");
        assert_eq!(failed[0].meeting, (3213, 3429, 0));
        assert!(store.failed_deals_since(201)?.is_empty());
        Ok(())
    }
}
