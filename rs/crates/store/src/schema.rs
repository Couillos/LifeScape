#![forbid(unsafe_code)]
//! Versioned schema. Migration 1 creates every table listed in lib.rs docs.
//! Later changes add a new migration step, never edit an applied one.

use crate::Error;

/// Latest schema version this build understands.
pub const SCHEMA_VERSION: u32 = 2;

const MIGRATION_V1: &str = "
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
const MIGRATION_V2: &str = "
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
    Ok(())
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
                 'task_yields','accounts_econ','trends','holdings_hourly','holdings_daily')",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(table_count, 11);
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

    struct RemoveGuard {
        path: std::path::PathBuf,
    }

    impl Drop for RemoveGuard {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_file(&self.path);
        }
    }
}
