#![forbid(unsafe_code)]
//! The single writer of the database (lot 5.8 of the architecture plan;
//! analysis 2.2 (f)). One connection writes, one thread commits, readers
//! keep their own connections (`store_pool`) and never wait for a write.
//!
//! Before: the market held a `Mutex<Store>`; every read and every write of
//! the store went through it, each write committed (one journal sync,
//! `synchronous=FULL`, unchanged) while the lock was held, so a read waited
//! for the disk of someone else's write.
//!
//! Now: `write()` hands out the writer connection (still one holder at a
//! time, so a block of reads and writes under one guard stays atomic with
//! respect to the other writers). The guard does NOT commit when it
//! drops: it opens (or joins) the current GROUP, a transaction shared by
//! every writer that comes while it is open, and asks the committer thread
//! for its commit. The committer takes the connection once, commits the
//! whole group with ONE sync and wakes everybody:
//!
//! - a write acknowledged to its caller is committed: `WriteGuard::drop`
//!   returns only once the group holding the write is committed, or has
//!   failed (`StoreWriter::failed_commits`, a `store-writer:` log line
//!   naming the number of writes lost; `WriteGuard::commit` returns the
//!   error to a caller that wants it). Nothing is acknowledged early.
//! - a caller reading its own write afterwards, through any connection
//!   (the pool included), sees it: the ack came after the commit.
//! - inside a group the writer's own reads see its uncommitted rows (same
//!   connection); other connections see them after the commit only.
//! - `batch()` marks a pass of many writes by one thread (the market
//!   reprice pass): its guards do not wait one by one, the batch waits
//!   once at its end. Another thread's write may commit the group
//!   earlier; the rows are the same.
//!
//! `shared(path)` is the process's writer of the file once the daemon
//! registered it (`register`); without one (tests, offline commands)
//! `write_to` falls back to a pooled connection that commits by itself.

use lifescape_store::{Error, Store};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

use crate::store_pool::PooledStore;

struct State {
    store: Store,
    /// A group (transaction) is open on the connection.
    open: bool,
    /// Writes (guards that changed rows) so far; a guard's ticket.
    written: u64,
    /// Highest ticket committed.
    committed: u64,
    /// Highest ticket lost to a failed commit, and why.
    failed_upto: u64,
    failure: String,
    /// Someone waits for the committer.
    requested: bool,
    closing: bool,
}

struct Inner {
    path: PathBuf,
    state: Mutex<State>,
    /// Waiters sleep here until their ticket is committed or failed.
    done: Condvar,
    /// The committer sleeps here until a commit is requested.
    wake: Condvar,
    groups: AtomicU64,
    grouped_writes: AtomicU64,
    failed: AtomicU64,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The writer of one database file.
pub struct StoreWriter {
    inner: Arc<Inner>,
}

thread_local! {
    /// Depth of the batches this thread has open (`StoreWriter::batch`).
    static BATCH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl StoreWriter {
    /// Opens the file (migrations applied here, once) and starts its
    /// committer thread, which ends with the last handle.
    pub fn open(path: &Path) -> Result<Arc<StoreWriter>, Error> {
        let store = Store::open(path)?;
        crate::store_pool::mark_migrated(path);
        let state = State { store, open: false, written: 0, committed: 0, failed_upto: 0, failure: String::new(), requested: false, closing: false };
        let inner = Arc::new(Inner {
            path: path.to_path_buf(),
            state: Mutex::new(state),
            done: Condvar::new(),
            wake: Condvar::new(),
            groups: AtomicU64::new(0),
            grouped_writes: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        });
        let committer = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("store-writer".to_string())
            .spawn(move || commit_loop(&committer))
            .map_err(|error| Error::Io(format!("store-writer thread: {error}")))?;
        Ok(Arc::new(StoreWriter { inner }))
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The writer connection, for one block of reads and writes. At most
    /// one holder at a time; the commit is requested and awaited when the
    /// guard drops (module doc).
    pub fn write(&self) -> WriteGuard<'_> {
        let mut state = self.inner.lock();
        let before = state.store.changes_made();
        match state.store.begin_group() {
            Ok(true) => state.open = true,
            Ok(false) => state.open = true,
            Err(error) => crate::logln!("store-writer: cannot open a write group: {error}"),
        }
        WriteGuard { writer: &self.inner, state: Some(state), before }
    }

    /// A reader's connection: the calling thread's pooled one.
    pub fn read(&self) -> Result<PooledStore, Error> {
        crate::store_pool::open(&self.inner.path)
    }

    /// Marks a pass of many writes by this thread: its guards do not wait
    /// one by one; the batch commits and waits once, when dropped.
    pub fn batch(&self) -> Batch<'_> {
        BATCH.with(|depth| depth.set(depth.get() + 1));
        Batch { writer: self }
    }

    /// Commits whatever is open and waits for it.
    pub fn flush(&self) -> Result<(), String> {
        let state = self.inner.lock();
        let ticket = state.written;
        let (_state, verdict) = await_commit(&self.inner, state, ticket);
        verdict
    }

    /// `(groups committed, writes in them)` since the start.
    pub fn group_stats(&self) -> (u64, u64) {
        (self.inner.groups.load(Ordering::Relaxed), self.inner.grouped_writes.load(Ordering::Relaxed))
    }

    /// Commits lost since the start (each also logged).
    pub fn failed_commits(&self) -> u64 {
        self.inner.failed.load(Ordering::Relaxed)
    }
}

impl Drop for StoreWriter {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        state.closing = true;
        self.inner.wake.notify_all();
    }
}

/// Waits until `ticket` is committed or failed, asking the committer for
/// it; hands the lock back with the verdict.
fn await_commit<'a>(inner: &'a Inner, mut state: MutexGuard<'a, State>, ticket: u64) -> (MutexGuard<'a, State>, Result<(), String>) {
    loop {
        if state.failed_upto >= ticket {
            let failure = state.failure.clone();
            return (state, Err(failure));
        }
        if state.committed >= ticket {
            return (state, Ok(()));
        }
        if !state.open {
            // Cannot happen (a group with pending writes stays open until
            // the committer closes it); loud rather than a silent ack.
            return (state, Err("write group closed before its commit".to_string()));
        }
        state.requested = true;
        inner.wake.notify_one();
        state = inner.done.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

fn commit_loop(inner: &Arc<Inner>) {
    let mut state = inner.lock();
    loop {
        while !state.requested && !state.closing {
            state = inner.wake.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        let closing = state.closing;
        if state.open {
            let writes = state.written - state.committed;
            match state.store.commit_batch() {
                Ok(()) => {
                    state.committed = state.written;
                    inner.groups.fetch_add(1, Ordering::Relaxed);
                    inner.grouped_writes.fetch_add(writes, Ordering::Relaxed);
                }
                Err(error) => {
                    state.store.rollback_batch();
                    state.failed_upto = state.written;
                    state.committed = state.written;
                    state.failure = format!("commit failed: {error}");
                    inner.failed.fetch_add(1, Ordering::Relaxed);
                    crate::logln!("store-writer: commit failed, {writes} write(s) lost: {error}");
                }
            }
            state.open = false;
        }
        state.requested = false;
        inner.done.notify_all();
        if closing {
            return;
        }
    }
}

/// The writer connection under the single-holder lock (see `write`).
pub struct WriteGuard<'a> {
    writer: &'a Arc<Inner>,
    state: Option<MutexGuard<'a, State>>,
    before: u64,
}

impl Deref for WriteGuard<'_> {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.state.as_ref().expect("guard holds the state until drop").store
    }
}

impl DerefMut for WriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.state.as_mut().expect("guard holds the state until drop").store
    }
}

impl WriteGuard<'_> {
    /// Ends the guard and returns the commit's verdict (drop only logs it).
    pub fn commit(mut self) -> Result<(), String> {
        self.finish()
    }

    fn finish(&mut self) -> Result<(), String> {
        let Some(mut state) = self.state.take() else { return Ok(()) };
        let changed = state.store.changes_made() != self.before;
        if !changed {
            // Only reads: when this guard opened an empty group, close it
            // at once (no sync, nothing to wait for).
            if state.open && state.written == state.committed {
                let _ = state.store.commit_batch();
                state.open = false;
            }
            return Ok(());
        }
        state.written += 1;
        let ticket = state.written;
        if !state.open {
            // No group could be opened: the statements committed by
            // themselves (the old behaviour).
            state.committed = state.written;
            return Ok(());
        }
        if BATCH.with(|depth| depth.get()) > 0 {
            return Ok(());
        }
        let inner: &Inner = self.writer;
        let (_state, verdict) = await_commit(inner, state, ticket);
        verdict
    }
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            crate::logln!("store-writer: a write was not committed: {error}");
        }
    }
}

/// Does this thread have a batch open (`StoreWriter::batch`)? Its reads
/// must go through the writer connection to see its own uncommitted rows.
pub fn in_batch() -> bool {
    BATCH.with(|depth| depth.get()) > 0
}

/// A pass of many writes by one thread (see `StoreWriter::batch`).
pub struct Batch<'a> {
    writer: &'a StoreWriter,
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        BATCH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        if let Err(error) = self.writer.flush() {
            crate::logln!("store-writer: a batch was not committed: {error}");
        }
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<StoreWriter>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<StoreWriter>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Makes `writer` the process's writer of its file (the daemon, once).
pub fn register(writer: &Arc<StoreWriter>) {
    registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(writer.path().to_path_buf(), Arc::clone(writer));
}

/// The registered writer of `path`, if the daemon registered one.
pub fn shared(path: &Path) -> Option<Arc<StoreWriter>> {
    registry().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(path).cloned()
}

/// Runs a write on the file: through the registered single writer when
/// there is one (grouped, acknowledged after the commit), else on this
/// thread's pooled connection (each statement commits by itself, as
/// before).
pub fn write_to<R>(path: &Path, write: impl FnOnce(&Store) -> R) -> Result<R, Error> {
    match shared(path) {
        Some(writer) => {
            let guard = writer.write();
            let result = write(&guard);
            guard.commit().map_err(Error::Sql)?;
            Ok(result)
        }
        None => {
            let store = crate::store_pool::open(path)?;
            Ok(write(&store))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_store::Item;

    fn temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-writer-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("w.db")
    }

    fn item(id: i64) -> Item {
        Item { item_id: id, name: format!("item {id}"), shop_price: 2, alch_value: 1 }
    }

    fn count(path: &Path) -> i64 {
        let reader = crate::store_pool::open(path).expect("reader");
        (1..=1000).filter(|id| reader.item_id_by_name(&format!("item {id}")).ok().flatten().is_some()).count() as i64
    }

    /// A write acknowledged to its caller is on disk and visible to a
    /// reader's own connection at once (the ack came after the commit).
    #[test]
    fn an_acknowledged_write_is_committed_and_visible_to_readers() {
        let path = temp_db("ack");
        let writer = StoreWriter::open(&path).expect("open");
        {
            let guard = writer.write();
            guard.upsert_item(&item(1)).expect("write");
        }
        assert_eq!(count(&path), 1, "visible through another connection once the guard dropped");
        writer.write().upsert_item(&item(2)).expect("write");
        assert_eq!(count(&path), 2);
        assert_eq!(writer.failed_commits(), 0);
    }

    /// Concurrent writers share groups: every row lands, with fewer
    /// commits than writes once they overlap.
    #[test]
    fn concurrent_writers_are_grouped_and_every_row_lands() {
        let path = temp_db("group");
        let writer = StoreWriter::open(&path).expect("open");
        let handles: Vec<_> = (0..8)
            .map(|thread| {
                let writer = Arc::clone(&writer);
                std::thread::spawn(move || {
                    for step in 0..25 {
                        let id = thread * 25 + step + 1;
                        let guard = writer.write();
                        guard.upsert_item(&item(id)).expect("write");
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread");
        }
        assert_eq!(count(&path), 200, "every acknowledged write is on disk");
        let (groups, writes) = writer.group_stats();
        assert_eq!(writes, 200);
        assert!(groups <= writes, "never more commits than writes ({groups}/{writes})");
    }

    /// A batch commits once at its end; the rows are all there after it.
    #[test]
    fn a_batch_waits_once_at_its_end() {
        let path = temp_db("batch");
        let writer = StoreWriter::open(&path).expect("open");
        {
            let _batch = writer.batch();
            for id in 1..=10 {
                writer.write().upsert_item(&item(id)).expect("write");
            }
            let (groups, _) = writer.group_stats();
            assert_eq!(groups, 0, "nothing committed while the batch runs");
        }
        assert_eq!(count(&path), 10);
        assert_eq!(writer.group_stats(), (1, 10), "one group, ten writes");
    }

    /// The writer's own reads see its uncommitted rows inside the guard.
    #[test]
    fn a_guard_reads_its_own_write() {
        let path = temp_db("own");
        let writer = StoreWriter::open(&path).expect("open");
        let guard = writer.write();
        guard.upsert_item(&item(7)).expect("write");
        assert!(guard.item_id_by_name("item 7").expect("read").is_some());
        guard.commit().expect("commit");
    }

    /// Without a registered writer `write_to` uses a pooled connection
    /// and the row is there at once; with one it goes through the writer.
    #[test]
    fn write_to_falls_back_to_the_pool_and_uses_the_writer_when_registered() {
        let path = temp_db("shared");
        drop(Store::open(&path).expect("create"));
        write_to(&path, |store| store.upsert_item(&item(1))).expect("pool").expect("write");
        assert_eq!(count(&path), 1);
        let writer = StoreWriter::open(&path).expect("open");
        register(&writer);
        write_to(&path, |store| store.upsert_item(&item(2))).expect("writer").expect("write");
        assert_eq!(count(&path), 2);
        assert_eq!(writer.group_stats().1, 1, "the second write went through the writer");
    }

    /// One sequence of orders, deals, executions, reports and cancels, all
    /// with explicit clocks (what the market and the daemon write).
    fn scenario(store: &Store) {
        use lifescape_store::{AccountEcon, DealStatus, NewDeal, NewExecution, NewOrder, OrderSide};
        for id in 1..=3 {
            store.upsert_item(&item(id)).expect("item");
        }
        for (index, name) in ["ann", "bob", "cy"].iter().enumerate() {
            store
                .upsert_account(&AccountEcon { account: name.to_string(), iron: false, deals_settled: index as i64, deals_failed: 0, market_affinity: 0.5, price_patience: 0.25, archetype: "market".to_string() })
                .expect("account");
        }
        let mut orders = Vec::new();
        for step in 0..12i64 {
            let side = if step % 2 == 0 { OrderSide::Sell } else { OrderSide::Buy };
            let order = NewOrder {
                account: ["ann", "bob", "cy"][(step % 3) as usize].to_string(),
                side,
                item_id: 1 + step % 3,
                quantity: 10 + step,
                price: 100 + step,
                dormant: false,
                clock: 1_000 + step,
                replaces: None,
                motive: String::new(),
                ceiling: None,
                ask_limit: None,
            };
            orders.push(store.place_order(&order).expect("order"));
        }
        for pair in 0..4i64 {
            let deal = store
                .create_deal(&NewDeal { buyer: "bob".into(), seller: "ann".into(), item_id: 1, quantity: 5, price: 120 + pair, meeting_x: 3213, meeting_y: 3429, meeting_level: 0, deadline_tick: 900, clock: 2_000 + pair, taker_side: OrderSide::Buy })
                .expect("deal");
            for (offset, status) in [DealStatus::Convened, DealStatus::BothArrived, DealStatus::Trading, DealStatus::Settled].into_iter().enumerate() {
                store.set_deal_status(deal, status, "", 2_100 + pair * 10 + offset as i64).expect("status");
            }
            store.record_execution(&NewExecution { item_id: 1, quantity: 5, price: 120 + pair, clock: 2_200 + pair, server_tick: 500 + pair, taker_side: Some(OrderSide::Buy) }).expect("execution");
        }
        for order in orders.iter().step_by(3) {
            store.cancel_order(*order, 3_000).expect("cancel");
        }
        for step in 0..3 {
            store
                .record_task_report(&lifescape_store::NewTaskReport { account: "ann".into(), node_id: format!("n{step}"), method: "mine".into(), script: "mine_ore".into(), ticks: 100 + step, stop: "goal_reached".into(), context_level: 1, clock: 4_000 + step })
                .expect("report");
        }
    }

    /// Lot 5.8 proof: the same sequence of orders, deals, executions,
    /// reports and cancels leaves the same rows whether it is applied
    /// straight on a store, through the single writer in one guard (one
    /// group, acknowledged after its commit, read back through a reader's
    /// connection), or inside a batch.
    #[test]
    fn the_writer_leaves_the_same_rows_as_direct_writes() {
        let direct_path = temp_db("replay-direct");
        let direct = Store::open(&direct_path).expect("direct");
        scenario(&direct);
        let want = direct.table_digest().expect("digest");
        assert!(want.get("orders").is_some_and(|rows| rows.len() == 12) && want.get("deals").is_some_and(|rows| rows.len() == 4), "the scenario reaches the tables");

        let acked_path = temp_db("replay-acked");
        let acked = StoreWriter::open(&acked_path).expect("writer");
        scenario(&acked.write());
        acked.flush().expect("flush");
        assert_eq!(crate::store_pool::open(&acked_path).expect("reader").table_digest().expect("digest"), want, "one guard for the whole sequence");

        let each_path = temp_db("replay-each");
        let each = StoreWriter::open(&each_path).expect("writer");
        {
            let _batch = each.batch();
            scenario(&each.write());
        }
        assert_eq!(crate::store_pool::open(&each_path).expect("reader").table_digest().expect("digest"), want, "inside a batch");
        assert_eq!(each.failed_commits(), 0);
    }
}
