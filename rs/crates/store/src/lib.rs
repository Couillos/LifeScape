#![forbid(unsafe_code)]
//! Typed SQLite persistence for the market and the yield measurements.
//!
//! Single opener rule: only the daemon opens the database file
//! (`run/lifescape.db`). Session workers never touch this crate; they talk
//! to the daemon through `run/market.sock`. There is no multi-process
//! locking: opening the same file twice is a usage error, not supported.
//!
//! The public API exposes plain project types only. No SQL string and no
//! rusqlite type leaks through these boundaries. Timestamps are unix clock
//! seconds (`clock`); durations measured in server ticks keep a separate
//! `server_tick` column wherever both scales matter.
//!
//! Candles are a cache: `executions` is the source of truth and any candle
//! set can be rebuilt with [`Store::rebuild_candles`].

pub mod account_views;
pub mod amends;
pub mod live_guard;
pub mod balance;
pub mod candle_chain;
pub mod candles;
pub mod deals;
pub mod economy;
pub mod economy_report;
pub mod errors;
pub mod holdings;
pub mod items;
mod levels;
#[cfg(test)]
mod levels_tests;
pub mod lineage;
pub mod market_criteria;
pub mod measures;
pub mod orders;
pub mod outcomes;
pub mod prices;
mod provenance_v8;
pub mod reports;
pub mod retention;
pub mod schema;
pub mod work_gains;
pub mod xp_rates;

pub use amends::OrderAmend;
pub use balance::{stock_days, MethodWork, UnsoldStock};
pub use candle_chain::chain_candles;
pub use candles::{Candle, Execution, NewExecution, YieldCandleResolution};
pub use deals::{Deal, DealStatus, NewDeal};
pub use retention::ExpiredOrders;
pub use economy::{AccountEcon, Trend};
pub use economy_report::{laspeyres, CoinFlowTotal, ItemFills};
pub use errors::{ErrorFilter, ErrorRecord, FailedDeal, TaskOutcome};
pub use items::Item;
pub use lineage::{LineageLink, LineageRow, AMEND_WINDOW_SECS};
pub use market_criteria::MarketCriteria;
pub use measures::{PurgeCounts, ReportMeasure, StoredMeasure};
pub use orders::{NewOrder, Order, OrderSide, OrderStatus, AMENDED_REASON, BOOKKEEPING_REASONS, MERGED_REASON, REGROUPED_REASON};
pub use reports::{NewTaskReport, NewTaskYield, TaskReport, TaskYield, YieldAggregate};
pub use work_gains::AccountWork;
pub use xp_rates::{BucketWork, SkillBucket};

use std::path::Path;

/// Typed store error. Every variant carries the context needed to tell a
/// corrupt database apart from a caller bug.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// SQLite failure, with the operation being attempted.
    Sql(String),
    /// Filesystem failure (missing directory, permissions).
    Io(String),
    /// Database file migrated by a newer binary; refusing to guess.
    SchemaMismatch { found: u32, expected: u32 },
    /// Lookup of a row that does not exist.
    NotFound(String),
    /// Caller value rejected before touching the database.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sql(detail) => write!(formatter, "store sql: {detail}"),
            Error::Io(detail) => write!(formatter, "store io: {detail}"),
            Error::SchemaMismatch { found, expected } => write!(
                formatter,
                "store schema mismatch: file is version {found}, this build supports {expected}"
            ),
            Error::NotFound(detail) => write!(formatter, "store not found: {detail}"),
            Error::Invalid(detail) => write!(formatter, "store invalid: {detail}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(source: rusqlite::Error) -> Self {
        Error::Sql(source.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Error::Io(source.to_string())
    }
}

fn is_busy(source: &rusqlite::Error) -> bool {
    matches!(
        source,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(failure.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// An atomic scope opened by `Store::open_scope`.
pub(crate) struct Scope {
    nested: bool,
}

/// SQLite backing store. Open once per process (the daemon).
pub struct Store {
    connection: rusqlite::Connection,
    views: std::cell::RefCell<account_views::AccountViews>,
}

impl Store {
    /// Opens (creating it) the database file, enables WAL and applies any
    /// pending migrations. One opener per file: the daemon.
    pub fn open(path: &Path) -> Result<Self, Error> {
        live_guard::assert_not_live(path);
        let connection =
            rusqlite::Connection::open(path).map_err(|source| Error::Sql(source.to_string()))?;
        Self::initialize(connection)
    }

    /// Opens a file an earlier `Store::open` of this process already
    /// migrated: the connection, the busy wait and the foreign keys, plus a
    /// cheap check that the file is at this build's schema (otherwise
    /// `SchemaMismatch`, the caller then falls back to `open`). No WAL
    /// switch (persistent in the file), no migration, no index statement:
    /// what long-lived daemon connections use (2026-11-23).
    pub fn open_migrated(path: &Path) -> Result<Self, Error> {
        live_guard::assert_not_live(path);
        let connection = rusqlite::Connection::open(path).map_err(|source| Error::Sql(source.to_string()))?;
        connection.busy_timeout(schema::BUSY_WAIT)?;
        connection.set_prepared_statement_cache_capacity(128);
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        let found = schema::current_version(&connection)?;
        if found != schema::SCHEMA_VERSION {
            return Err(Error::SchemaMismatch { found, expected: schema::SCHEMA_VERSION });
        }
        Ok(Self { connection, views: account_views::empty() })
    }

    /// In-memory database with the same schema. Tests and offline tools.
    pub fn open_in_memory() -> Result<Self, Error> {
        let connection = rusqlite::Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(connection: rusqlite::Connection) -> Result<Self, Error> {
        // A concurrent opener (migration, write batch) is waited for,
        // never a raw SQLITE_BUSY (2026-11-16 migration race).
        connection.busy_timeout(schema::BUSY_WAIT)?;
        connection.set_prepared_statement_cache_capacity(128);
        Self::enable_wal(&connection)?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        // Also ensures the reports-by-time index (`schema::REPORT_CLOCK_INDEX`:
        // measured gains, the brain's likely shop stock) without a version change.
        schema::apply_migrations(&connection)?;
        Ok(Self { connection, views: account_views::empty() })
    }

    /// Switches to WAL. On a fresh file the switch returns SQLITE_BUSY at
    /// once when another opener holds the lock (the busy handler is not
    /// consulted), so retry until `schema::BUSY_WAIT` elapses, then fail typed.
    fn enable_wal(connection: &rusqlite::Connection) -> Result<(), Error> {
        let started = std::time::Instant::now();
        loop {
            match connection.execute_batch("PRAGMA journal_mode=WAL;") {
                Ok(()) => return Ok(()),
                Err(source) if is_busy(&source) && started.elapsed() < schema::BUSY_WAIT => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(source) => return Err(Error::Sql(format!("wal switch: {source}"))),
            }
        }
    }

    /// Opens one write batch (a single transaction for a pass of many
    /// small writes: the market reprice pass, 2026-11-14). Returns false
    /// when a batch is already open (the caller then commits nothing).
    pub fn begin_batch(&self) -> Result<bool, Error> {
        if !self.connection.is_autocommit() {
            return Ok(false);
        }
        self.connection.execute_batch("BEGIN")?;
        Ok(true)
    }

    /// Opens the write group of the single writer (`BEGIN IMMEDIATE`: the
    /// write lock is taken now, waiting out another connection's commit
    /// instead of failing at the first statement). False when one is open.
    pub fn begin_group(&self) -> Result<bool, Error> {
        if !self.connection.is_autocommit() {
            return Ok(false);
        }
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(true)
    }

    /// Rows this connection has changed since it opened (SQLite's
    /// `total_changes`): moves with every INSERT/UPDATE/DELETE.
    pub fn changes_made(&self) -> u64 {
        self.connection.total_changes()
    }

    /// Commits the batch `begin_batch` opened.
    pub fn commit_batch(&self) -> Result<(), Error> {
        if self.connection.is_autocommit() {
            return Ok(());
        }
        self.connection.execute_batch("COMMIT")?;
        Ok(())
    }

    /// `Connection::execute` through the statement cache (one parse per
    /// distinct text and connection, not per call).
    pub(crate) fn run(&self, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<usize> {
        self.connection.prepare_cached(sql)?.execute(params)
    }

    /// Rewrites the file without its free pages (`VACUUM`) and empties the
    /// WAL. Needs the file to itself and room for a full copy; never run
    /// by the daemon (`orchestrator store-compact`, daemon stopped).
    pub fn compact(&self) -> Result<(), Error> {
        self.connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// Rolls back the batch `begin_batch` opened (a failed multi-row write
    /// leaves nothing behind). No-op when no batch is open.
    pub fn rollback_batch(&self) {
        if !self.connection.is_autocommit() {
            let _ignored = self.connection.execute_batch("ROLLBACK");
        }
    }

    /// Opens an atomic scope: a transaction when none is open, else a
    /// savepoint inside the open one (the single writer keeps a group of
    /// writes open across calls, `orch-core::store_writer`).
    pub(crate) fn open_scope(&self) -> Result<Scope, Error> {
        if self.connection.is_autocommit() {
            self.connection.execute_batch("BEGIN IMMEDIATE")?;
            Ok(Scope { nested: false })
        } else {
            self.connection.execute_batch("SAVEPOINT scope")?;
            Ok(Scope { nested: true })
        }
    }

    /// Ends the scope: commit (or release) when `ok`, else roll it back.
    pub(crate) fn close_scope(&self, scope: Scope, ok: bool) -> Result<(), Error> {
        match (scope.nested, ok) {
            (false, true) => self.connection.execute_batch("COMMIT")?,
            (false, false) => {
                let _ignored = self.connection.execute_batch("ROLLBACK");
            }
            (true, true) => self.connection.execute_batch("RELEASE scope")?,
            (true, false) => {
                let _ignored = self.connection.execute_batch("ROLLBACK TO scope; RELEASE scope");
            }
        }
        Ok(())
    }

    /// True when no transaction is open (a connection kept for reuse must
    /// be idle).
    pub fn is_idle(&self) -> bool {
        self.connection.is_autocommit()
    }

    /// Runs `write` in one transaction (joins the caller's batch when one
    /// is already open): all its statements commit together, one journal
    /// sync instead of one per statement, and a failure leaves nothing.
    pub fn in_batch<T, E: From<Error>>(&self, write: impl FnOnce(&Store) -> Result<T, E>) -> Result<T, E> {
        // A transaction when none is open, else a savepoint inside the open
        // one (a batch, or the single writer's group): either way a failure
        // leaves none of this call's rows.
        let scope = self.open_scope()?;
        let written = write(self);
        self.close_scope(scope, written.is_ok())?;
        written
    }

    /// Every row of every table as text, per table, rows sorted by column:
    /// what the equivalence tests compare (same sequence of writes, same
    /// rows, whatever the commit grouping). Not for production reads.
    pub fn table_digest(&self) -> Result<std::collections::BTreeMap<String, Vec<String>>, Error> {
        let names: Vec<String> = {
            let mut statement = self.connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut digest = std::collections::BTreeMap::new();
        for name in names {
            let columns = self.connection.prepare(&format!("SELECT * FROM \"{name}\""))?.column_count();
            let order: Vec<String> = (1..=columns).map(|column| column.to_string()).collect();
            let mut statement = self.connection.prepare(&format!("SELECT * FROM \"{name}\" ORDER BY {}", order.join(", ")))?;
            let rows: Vec<String> = statement
                .query_map([], |row| {
                    let mut cells = Vec::with_capacity(columns);
                    for column in 0..columns {
                        cells.push(match row.get_ref(column)? {
                            rusqlite::types::ValueRef::Null => "NULL".to_string(),
                            rusqlite::types::ValueRef::Integer(value) => value.to_string(),
                            rusqlite::types::ValueRef::Real(value) => value.to_string(),
                            rusqlite::types::ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
                            rusqlite::types::ValueRef::Blob(value) => format!("blob{}", value.len()),
                        });
                    }
                    Ok(cells.join("|"))
                })?
                .collect::<Result<_, _>>()?;
            digest.insert(name, rows);
        }
        Ok(digest)
    }

    /// Schema version of the open file (equals the latest migration).
    pub fn schema_version(&self) -> Result<u32, Error> {
        schema::current_version(&self.connection)
    }
}

#[cfg(test)]
pub(crate) fn ensure_test_item(store: &Store, item_id: i64) -> Result<(), Error> {
    store.upsert_item(&Item {
        item_id,
        name: format!("Test item {item_id}"),
        shop_price: 2,
        alch_value: 1,
    })
}

#[cfg(test)]
pub(crate) fn temp_database_path(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lifescape-store-test-{}-{}-{stamp}.db",
        std::process::id(),
        tag
    ));
    path
}
