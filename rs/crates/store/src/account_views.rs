//! Read cache of the two per-account market reads the brain repeats many
//! times per pass (`account_orders`, `open_deals_for_account`: locks,
//! committed coins, listings, upkeep facts), keyed by the store's change
//! stamp so a cached answer is never older than the rows.
//!
//! Stamp = (rows this connection changed since it opened, SQLite's
//! `data_version`). The first moves on any INSERT/UPDATE/DELETE of this
//! connection (triggers included), the second on any commit of ANOTHER
//! connection of the same file: together they change whenever the rows a
//! query would return may have changed. A changed stamp empties the whole
//! cache (one stamp per cache, 74 accounts of a few rows each). A change
//! after the stamp was read is seen by the next read, exactly like a change
//! just after an uncached read.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::deals::Deal;
use crate::orders::Order;
use crate::{Error, Store};

static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// `(served from the cache, read from the database)` since the process
/// started, both reads together (`brain tally`).
pub fn account_view_stats() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

#[derive(Default)]
pub(crate) struct AccountViews {
    stamp: Option<(u64, i64)>,
    orders: HashMap<String, Vec<Order>>,
    deals: HashMap<String, Vec<Deal>>,
}

impl Store {
    /// Moves whenever the rows of any table may have changed (module doc).
    pub fn change_stamp(&self) -> Result<(u64, i64), Error> {
        let mut statement = self.connection.prepare_cached("PRAGMA data_version")?;
        let version: i64 = statement.query_row([], |row| row.get(0))?;
        Ok((self.connection.total_changes(), version))
    }

    /// `change_stamp` as one number a view can be numbered by (changes of
    /// this connection plus commits of the others).
    pub fn generation(&self) -> Result<u64, Error> {
        let (changes, version) = self.change_stamp()?;
        Ok(changes.wrapping_add((version as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
    }

    fn fresh_views(&self) -> Result<std::cell::RefMut<'_, AccountViews>, Error> {
        let stamp = self.change_stamp()?;
        let mut views = self.views.borrow_mut();
        if views.stamp != Some(stamp) {
            views.orders.clear();
            views.deals.clear();
            views.stamp = Some(stamp);
        }
        Ok(views)
    }

    /// Every non-final order of one account (cached by change stamp).
    pub fn account_orders(&self, account: &str) -> Result<Vec<Order>, Error> {
        {
            let views = self.fresh_views()?;
            if let Some(rows) = views.orders.get(account) {
                HITS.fetch_add(1, Ordering::Relaxed);
                return Ok(rows.clone());
            }
        }
        // The stamp was read before the query: a change in between is a
        // newer row under an older stamp, re-read at the next call.
        let stamp = self.change_stamp()?;
        let rows = self.fresh_account_orders(account)?;
        MISSES.fetch_add(1, Ordering::Relaxed);
        let mut views = self.views.borrow_mut();
        if views.stamp == Some(stamp) {
            views.orders.insert(account.to_string(), rows.clone());
        }
        Ok(rows)
    }

    /// Open deals where the account is buyer or seller (cached by change stamp).
    pub fn open_deals_for_account(&self, account: &str) -> Result<Vec<Deal>, Error> {
        {
            let views = self.fresh_views()?;
            if let Some(rows) = views.deals.get(account) {
                HITS.fetch_add(1, Ordering::Relaxed);
                return Ok(rows.clone());
            }
        }
        let stamp = self.change_stamp()?;
        let rows = self.fresh_open_deals_for_account(account)?;
        MISSES.fetch_add(1, Ordering::Relaxed);
        let mut views = self.views.borrow_mut();
        if views.stamp == Some(stamp) {
            views.deals.insert(account.to_string(), rows.clone());
        }
        Ok(rows)
    }
}

/// The cache is only the stored rows: it must agree with the database after
/// every kind of change (`views` tests in `orders`/`deals` modules own the
/// row fixtures; this one drives the stamp).
#[cfg(test)]
#[path = "account_views_tests.rs"]
mod tests;

/// For `RefCell` in `Store` (module-private constructor).
pub(crate) fn empty() -> RefCell<AccountViews> {
    RefCell::new(AccountViews::default())
}
