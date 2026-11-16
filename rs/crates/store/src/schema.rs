#![forbid(unsafe_code)]
//! Versioned schema. Migration 1 creates every table listed in lib.rs docs.
//! Later changes add a new migration step, never edit an applied one.

use crate::Error;

/// Latest schema version this build understands.
pub const SCHEMA_VERSION: u32 = 6;

pub(crate) const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS items(
    item_id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    shop_price INTEGER NOT NULL DEFAULT 0,
    alch_value INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS orders(
    order_id INTEGER PRIMARY KEY AUTOINCREMENT,
    account TEXT NOT NULL,
    side TEXT NOT NULL,
    item_id INTEGER NOT NULL REFERENCES items(item_id),
    quantity INTEGER NOT NULL,
    remaining INTEGER NOT NULL,
    price INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    dormant INTEGER NOT NULL DEFAULT 0,
    created_clock INTEGER NOT NULL,
    updated_clock INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_orders_item_status ON orders(item_id, status);
CREATE INDEX IF NOT EXISTS idx_orders_account ON orders(account, status);
CREATE TABLE IF NOT EXISTS deals(
    deal_id INTEGER PRIMARY KEY AUTOINCREMENT,
    buyer TEXT NOT NULL,
    seller TEXT NOT NULL,
    item_id INTEGER NOT NULL REFERENCES items(item_id),
    quantity INTEGER NOT NULL,
    price INTEGER NOT NULL,
    meeting_x INTEGER NOT NULL,
    meeting_y INTEGER NOT NULL,
    meeting_level INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'matched',
    fail_reason TEXT NOT NULL DEFAULT '',
    deadline_tick INTEGER NOT NULL,
    created_clock INTEGER NOT NULL,
    updated_clock INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_deals_buyer ON deals(buyer, status);
CREATE INDEX IF NOT EXISTS idx_deals_seller ON deals(seller, status);
CREATE TABLE IF NOT EXISTS executions(
    exec_id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id INTEGER NOT NULL REFERENCES items(item_id),
    quantity INTEGER NOT NULL,
    price INTEGER NOT NULL,
    clock INTEGER NOT NULL,
    server_tick INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_executions_item_clock ON executions(item_id, clock);
CREATE TABLE IF NOT EXISTS candles(
    item_id INTEGER NOT NULL REFERENCES items(item_id),
    resolution TEXT NOT NULL,
    window_start INTEGER NOT NULL,
    open_price INTEGER NOT NULL,
    high_price INTEGER NOT NULL,
    low_price INTEGER NOT NULL,
    close_price INTEGER NOT NULL,
    volume INTEGER NOT NULL,
    PRIMARY KEY(item_id, resolution, window_start)
);
CREATE TABLE IF NOT EXISTS task_reports(
    report_id INTEGER PRIMARY KEY AUTOINCREMENT,
    account TEXT NOT NULL,
    node_id TEXT NOT NULL,
    method TEXT NOT NULL,
    script TEXT NOT NULL,
    ticks INTEGER NOT NULL,
    stop TEXT NOT NULL,
    context_level INTEGER NOT NULL DEFAULT 0,
    created_clock INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_reports_method ON task_reports(method);
CREATE TABLE IF NOT EXISTS task_yields(
    yield_id INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    item_name TEXT NOT NULL DEFAULT '',
    provenance TEXT NOT NULL,
    quantity INTEGER NOT NULL DEFAULT 0,
    skill TEXT NOT NULL DEFAULT '',
    xp INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_task_yields_report ON task_yields(report_id);
CREATE INDEX IF NOT EXISTS idx_task_yields_item ON task_yields(item_name, provenance);
CREATE TABLE IF NOT EXISTS accounts_econ(
    account TEXT PRIMARY KEY,
    iron INTEGER NOT NULL DEFAULT 0,
    deals_settled INTEGER NOT NULL DEFAULT 0,
    deals_failed INTEGER NOT NULL DEFAULT 0,
    market_affinity REAL NOT NULL DEFAULT 0.5,
    price_patience REAL NOT NULL DEFAULT 0.5,
    archetype TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS trends(
    item_id INTEGER NOT NULL REFERENCES items(item_id),
    period TEXT NOT NULL,
    shortage REAL NOT NULL DEFAULT 0.0,
    momentum REAL NOT NULL DEFAULT 0.0,
    score REAL NOT NULL DEFAULT 0.0,
    computed_clock INTEGER NOT NULL,
    PRIMARY KEY(item_id, period)
);
";

/// Migration 2 (PRD 2, D48): item holdings move from the 30-minute JSON
/// sampler to SQLite. Hourly points (one per item and hour) are kept 8
/// days, daily points (last measure of each day) 400 days.
pub(crate) const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS holdings_hourly(
    item_id INTEGER NOT NULL,
    taken_at INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY(item_id, taken_at)
);
CREATE TABLE IF NOT EXISTS holdings_daily(
    item_id INTEGER NOT NULL,
    day_start INTEGER NOT NULL,
    taken_at INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY(item_id, day_start)
);
CREATE INDEX IF NOT EXISTS idx_holdings_hourly_time ON holdings_hourly(taken_at);
";

/// Migration 3 (errors page): one centralised error log fed by the
/// daemon at ingest time (task errors, nav dossiers, failed deals, brain
/// abandons). Pulled rows carry a unique `source_ref` (rescan-safe),
/// pushed rows leave it NULL. Retention is the collector's job.
pub(crate) const MIGRATION_V3: &str = "
CREATE TABLE IF NOT EXISTS error_events(
    error_id INTEGER PRIMARY KEY AUTOINCREMENT,
    clock INTEGER NOT NULL,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    account TEXT NOT NULL DEFAULT '',
    peer TEXT NOT NULL DEFAULT '',
    script TEXT NOT NULL DEFAULT '',
    args TEXT NOT NULL DEFAULT '',
    node_id TEXT NOT NULL DEFAULT '',
    method TEXT NOT NULL DEFAULT '',
    stop TEXT NOT NULL DEFAULT '',
    signature TEXT NOT NULL DEFAULT '',
    message TEXT NOT NULL DEFAULT '',
    has_pos INTEGER NOT NULL DEFAULT 0,
    pos_x INTEGER NOT NULL DEFAULT 0,
    pos_z INTEGER NOT NULL DEFAULT 0,
    level INTEGER NOT NULL DEFAULT 0,
    zone TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',
    dossier TEXT NOT NULL DEFAULT '',
    source_ref TEXT UNIQUE
);
CREATE INDEX IF NOT EXISTS idx_error_events_clock ON error_events(clock);
CREATE INDEX IF NOT EXISTS idx_error_events_account ON error_events(account, clock);
CREATE INDEX IF NOT EXISTS idx_error_events_source ON error_events(source, clock);
";

/// Migration 4 (market chart, buy/sell volume split): the side of the
/// order that consumed the book (taker, "buy"|"sell") rides each deal and
/// its settled execution; candles cache buy and sell volume. Rows written
/// before keep NULL (unknown side) and a 0/0 split: counted in `volume`
/// only, never guessed. Columns only, nothing dropped or rewritten; the
/// whole step is one transaction with its `user_version` bump.
pub(crate) const MIGRATION_V4: &str = "
ALTER TABLE deals ADD COLUMN taker_side TEXT;
ALTER TABLE executions ADD COLUMN taker_side TEXT;
ALTER TABLE candles ADD COLUMN buy_volume INTEGER NOT NULL DEFAULT 0;
ALTER TABLE candles ADD COLUMN sell_volume INTEGER NOT NULL DEFAULT 0;
PRAGMA user_version = 4;
";

/// Migration 5 (plan rendements R1, report contract 2): one measure, the
/// signed flow balance of a run. Reports gain their measure version (1:
/// the net-diff reports, purge target D8; 2: the ledger contract), the
/// planner's fine method and whether the launch carried it, site,
/// target, tool tier, combat style, work and pause ticks. Side tables:
/// every skill's level at the start, the signed flows by cause, the
/// XP-bearing units (product, input, cast, kill, damage) and the alerts
/// (unattributed flows, coherence residuals). Columns and tables only,
/// nothing dropped; one transaction with its version bump.
pub(crate) const MIGRATION_V5: &str = "
ALTER TABLE task_reports ADD COLUMN measure_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE task_reports ADD COLUMN planner_method TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN method_source TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN site TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN target TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN tool TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN combat_style TEXT NOT NULL DEFAULT '';
ALTER TABLE task_reports ADD COLUMN work_ticks INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_reports ADD COLUMN pause_ticks INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_task_reports_measure ON task_reports(measure_version);
CREATE INDEX IF NOT EXISTS idx_task_reports_planner ON task_reports(planner_method);
CREATE TABLE IF NOT EXISTS task_levels(
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    skill TEXT NOT NULL,
    level INTEGER NOT NULL,
    PRIMARY KEY(report_id, skill)
);
CREATE TABLE IF NOT EXISTS task_flows(
    flow_id INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    item_name TEXT NOT NULL,
    cause TEXT NOT NULL,
    quantity INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_flows_report ON task_flows(report_id);
CREATE INDEX IF NOT EXISTS idx_task_flows_item ON task_flows(item_name, cause);
CREATE TABLE IF NOT EXISTS task_units(
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    unit TEXT NOT NULL,
    subject TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    PRIMARY KEY(report_id, unit, subject)
);
CREATE TABLE IF NOT EXISTS task_alerts(
    alert_id INTEGER PRIMARY KEY AUTOINCREMENT,
    report_id INTEGER NOT NULL REFERENCES task_reports(report_id),
    kind TEXT NOT NULL,
    item_name TEXT NOT NULL,
    quantity INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_alerts_report ON task_alerts(report_id);
PRAGMA user_version = 5;
";

/// Reads the `user_version` pragma of an open connection.
pub fn current_version(connection: &rusqlite::Connection) -> Result<u32, Error> {
    let found: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|source| Error::Sql(source.to_string()))?;
    u32::try_from(found).map_err(|_| Error::Sql(format!("bad user_version {found}")))
}

/// Applies every migration newer than the file version, in order.
pub fn apply_migrations(connection: &rusqlite::Connection) -> Result<(), Error> {
    let found = current_version(connection)?;
    if found > SCHEMA_VERSION {
        return Err(Error::SchemaMismatch {
            found,
            expected: SCHEMA_VERSION,
        });
    }
    if found < 1 {
        connection
            .execute_batch(MIGRATION_V1)
            .map_err(|source| Error::Sql(source.to_string()))?;
        connection
            .execute_batch("PRAGMA user_version = 1")
            .map_err(|source| Error::Sql(source.to_string()))?;
    }
    if found < 2 {
        connection
            .execute_batch(MIGRATION_V2)
            .map_err(|source| Error::Sql(source.to_string()))?;
        connection
            .execute_batch("PRAGMA user_version = 2")
            .map_err(|source| Error::Sql(source.to_string()))?;
    }
    if found < 3 {
        connection
            .execute_batch(MIGRATION_V3)
            .map_err(|source| Error::Sql(source.to_string()))?;
        connection
            .execute_batch("PRAGMA user_version = 3")
            .map_err(|source| Error::Sql(source.to_string()))?;
    }
    if found < 4 {
        apply_in_transaction(connection, MIGRATION_V4, 4)?;
    }
    if found < 5 {
        apply_in_transaction(connection, MIGRATION_V5, 5)?;
    }
    if found < 6 {
        apply_lineage_v6(connection)?;
    }
    Ok(())
}

/// Migration 6 (plan marché M0, decision Q1): order lineage columns,
/// then the history reclassified (`lineage::reclassify_history`: an
/// amend is a replacement within 1 s, same account, object and side).
/// One transaction with its version bump: a failure leaves schema 5.
fn apply_lineage_v6(connection: &rusqlite::Connection) -> Result<(), Error> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|source| Error::Sql(format!("migration 6 begin: {source}")))?;
    let migrated = connection
        .execute_batch(crate::lineage::MIGRATION_V6_COLUMNS)
        .map_err(|source| Error::Sql(format!("migration 6: {source}")))
        .and_then(|()| crate::lineage::reclassify_history(connection))
        .and_then(|counts| {
            connection.execute_batch("PRAGMA user_version = 6").map_err(|source| Error::Sql(format!("migration 6 version: {source}")))?;
            Ok(counts)
        });
    if let Err(error) = migrated {
        let _ignored = connection.execute_batch("ROLLBACK");
        return Err(error);
    }
    connection
        .execute_batch("COMMIT")
        .map_err(|source| Error::Sql(format!("migration 6 commit: {source}")))
}

/// Runs one migration step and its version bump atomically: a failed
/// ALTER leaves the file at the previous version, never half-migrated.
fn apply_in_transaction(connection: &rusqlite::Connection, step: &str, version: u32) -> Result<(), Error> {
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|source| Error::Sql(format!("migration {version} begin: {source}")))?;
    if let Err(source) = connection.execute_batch(step) {
        let _ignored = connection.execute_batch("ROLLBACK");
        return Err(Error::Sql(format!("migration {version}: {source}")));
    }
    connection
        .execute_batch("COMMIT")
        .map_err(|source| Error::Sql(format!("migration {version} commit: {source}")))
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use crate::{Store, temp_database_path};

    #[test]
    fn migrations_create_tables_on_temp_file() -> Result<(), Error> {
        let path = temp_database_path("migrate");
        let _remove_guard = RemoveGuard { path: path.clone() };
        {
            let store = Store::open(&path)?;
            assert_eq!(store.schema_version()?, SCHEMA_VERSION);
            let table_count: i64 = store.connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN \
                 ('items','orders','deals','executions','candles','task_reports',\
                 'task_yields','accounts_econ','trends','holdings_hourly','holdings_daily',\
                 'error_events','task_levels','task_flows','task_units','task_alerts')",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(table_count, 16);
        }
        // Reopen keeps the version: migrations are idempotent.
        let reopened = Store::open(&path)?;
        assert_eq!(reopened.schema_version()?, SCHEMA_VERSION);
        Ok(())
    }

    /// A v1 file migrates to v2 without losing its market rows.
    #[test]
    fn v1_file_gains_holdings_tables() -> Result<(), Error> {
        use rusqlite::Connection;
        let path = temp_database_path("migrate-v1");
        let _remove_guard = RemoveGuard { path: path.clone() };
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch(super::MIGRATION_V1)?;
            connection.execute_batch("PRAGMA user_version = 1")?;
            connection.execute(
                "INSERT INTO items(item_id, name, shop_price, alch_value) VALUES (995, 'Coins', 0, 0)",
                [],
            )?;
        }
        let store = Store::open(&path)?;
        assert_eq!(store.schema_version()?, SCHEMA_VERSION);
        assert_eq!(
            store.get_item(995)?.map(|item| item.name),
            Some("Coins".to_string()),
            "v1 rows survive the v2 migration"
        );
        store.record_holdings(1_762_675_200, &[(995, 7)])?;
        assert_eq!(store.hourly_points(Some(995))?, vec![(1_762_675_200, 7)]);
        Ok(())
    }

    /// A v3 file (live base before the volume split) gains the taker
    /// columns without losing a row: old deals and executions read an
    /// unknown side, old candles a zero buy/sell split.
    #[test]
    fn v3_file_gains_taker_columns_without_loss() -> Result<(), Error> {
        use rusqlite::Connection;
        let path = temp_database_path("migrate-v3");
        let _remove_guard = RemoveGuard { path: path.clone() };
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch(super::MIGRATION_V1)?;
            connection.execute_batch(super::MIGRATION_V2)?;
            connection.execute_batch(super::MIGRATION_V3)?;
            connection.execute_batch("PRAGMA user_version = 3")?;
            connection.execute_batch(
                "INSERT INTO items(item_id, name, shop_price, alch_value) VALUES (314, 'Feather', 2, 1);
                 INSERT INTO deals(buyer, seller, item_id, quantity, price, meeting_x, meeting_y, meeting_level,
                     status, fail_reason, deadline_tick, created_clock, updated_clock)
                     VALUES ('b', 's', 314, 5, 3, 0, 0, 0, 'settled', '', 0, 36000, 36100);
                 INSERT INTO executions(item_id, quantity, price, clock, server_tick) VALUES (314, 5, 3, 36100, 36100);
                 INSERT INTO candles(item_id, resolution, window_start, open_price, high_price, low_price, close_price, volume)
                     VALUES (314, '1h', 36000, 3, 3, 3, 3, 5);",
            )?;
        }
        let mut store = Store::open(&path)?;
        assert_eq!(store.schema_version()?, SCHEMA_VERSION);
        let deal = store.get_deal(1)?.expect("old deal kept");
        assert_eq!((deal.quantity, deal.taker_side), (5, None));
        let fills = store.executions_for_item(314, 0, i64::MAX)?;
        assert_eq!(fills.iter().map(|fill| (fill.quantity, fill.taker_side)).collect::<Vec<_>>(), vec![(5, None)]);
        let cached = store.candles_for(314, crate::YieldCandleResolution::Hour1, 0, i64::MAX)?;
        assert_eq!(
            cached.iter().map(|candle| (candle.volume, candle.buy_volume, candle.sell_volume)).collect::<Vec<_>>(),
            vec![(5, 0, 0)]
        );
        let rebuilt = store.rebuild_candles(314, crate::YieldCandleResolution::Hour1)?;
        assert_eq!(rebuilt, cached, "the cache already agrees with the unknown side");
        drop(store);
        let reopened = Store::open(&path)?;
        assert_eq!(reopened.schema_version()?, SCHEMA_VERSION, "idempotent reopen");
        Ok(())
    }

    /// A v4 file (live base before the yield contract) gains the v2
    /// measure columns: its reports read measure version 1 (the purge
    /// target), nothing lost.
    #[test]
    fn v4_file_marks_old_reports_measure_one() -> Result<(), Error> {
        use rusqlite::Connection;
        let path = temp_database_path("migrate-v4");
        let _remove_guard = RemoveGuard { path: path.clone() };
        {
            let connection = Connection::open(&path)?;
            for step in [super::MIGRATION_V1, super::MIGRATION_V2, super::MIGRATION_V3, super::MIGRATION_V4] {
                connection.execute_batch(step)?;
            }
            connection.execute_batch(
                "INSERT INTO task_reports(account, node_id, method, script, ticks, stop, context_level, created_clock)
                     VALUES ('t01', 'n1', 'fishing', 'fishing', 600, 'goal_reached', 5, 1);
                 INSERT INTO task_yields(report_id, item_name, provenance, quantity, skill, xp) VALUES (1, 'Raw shrimps', 'produced', 9, '', 0);",
            )?;
        }
        let store = Store::open(&path)?;
        assert_eq!(store.schema_version()?, SCHEMA_VERSION);
        let version: i64 = store.connection.query_row("SELECT measure_version FROM task_reports WHERE report_id = 1", [], |row| row.get(0))?;
        assert_eq!(version, 1);
        assert_eq!(store.yields_for_report(1)?.len(), 1, "v1 lines kept until the purge");
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
