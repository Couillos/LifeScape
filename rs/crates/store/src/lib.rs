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

pub mod candles;
pub mod deals;
pub mod economy;
pub mod holdings;
pub mod items;
pub mod orders;
pub mod reports;
pub mod schema;

pub use candles::{Candle, Execution, NewExecution, YieldCandleResolution};
pub use deals::{Deal, DealStatus, NewDeal};
pub use economy::{AccountEcon, Trend};
pub use items::Item;
pub use orders::{NewOrder, Order, OrderSide, OrderStatus};
pub use reports::{NewTaskReport, NewTaskYield, TaskReport, TaskYield, YieldAggregate};

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

/// SQLite backing store. Open once per process (the daemon).
pub struct Store {
    connection: rusqlite::Connection,
}

impl Store {
    /// Opens (creating it) the database file, enables WAL and applies any
    /// pending migrations. One opener per file: the daemon.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let connection =
            rusqlite::Connection::open(path).map_err(|source| Error::Sql(source.to_string()))?;
        Self::initialize(connection)
    }

    /// In-memory database with the same schema. Tests and offline tools.
    pub fn open_in_memory() -> Result<Self, Error> {
        let connection = rusqlite::Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(connection: rusqlite::Connection) -> Result<Self, Error> {
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        schema::apply_migrations(&connection)?;
        Ok(Self { connection })
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
