//! Long-lived store connections for the daemon's own threads
//! (2026-11-23). Before, ~20 places called `Store::open` for a single read
//! or write: a new connection, a WAL switch, a migration check and an index
//! statement every time (and one more every time, on the hot paths: each
//! task report, each launch pass).
//!
//! `open(path)` hands back a connection kept per thread and per file:
//! the first open of a file in the process is a full `Store::open`
//! (migrations applied once, there), every later one reuses the thread's
//! connection or opens a bare one (`Store::open_migrated`: a version check,
//! nothing else). The guard derefs to `Store` and returns the connection to
//! the thread's cache on drop, unless a transaction was left open on it or
//! the file was replaced on disk (then it is closed).

#![forbid(unsafe_code)]

use lifescape_store::{Error, Store};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Identity of a file on disk (device, inode): a path whose file was
/// deleted and recreated is another database.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|meta| (meta.dev(), meta.ino()))
}

/// Files this process already migrated through `Store::open`.
static MIGRATED: Mutex<Option<HashSet<(u64, u64)>>> = Mutex::new(None);

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, ((u64, u64), Store)>> = RefCell::new(HashMap::new());
}

/// A connection checked out of the thread's cache; derefs to the store.
pub struct PooledStore {
    path: PathBuf,
    id: Option<(u64, u64)>,
    store: Option<Store>,
}

impl Deref for PooledStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.store.as_ref().expect("store present until drop")
    }
}

impl Drop for PooledStore {
    fn drop(&mut self) {
        let (Some(store), Some(id)) = (self.store.take(), self.id) else { return };
        if store.is_idle() && identity(&self.path) == Some(id) {
            let path = self.path.clone();
            // The thread may be shutting down: a gone cache just closes it.
            let _ignored = CACHE.try_with(|cache| cache.borrow_mut().insert(path, (id, store)));
        }
    }
}

/// Records that this process already migrated `path` (the single writer
/// opens it with `Store::open`): the pool's first open on a reader thread
/// is then the cheap `open_migrated`, never a migration check that could
/// queue behind the writer's open group.
pub fn mark_migrated(path: &Path) {
    if let Some(id) = identity(path) {
        MIGRATED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get_or_insert_with(HashSet::new).insert(id);
    }
}

/// The daemon's connection to `path` for this thread (see the module doc).
pub fn open(path: &Path) -> Result<PooledStore, Error> {
    let id = identity(path);
    if let Some(id) = id {
        let cached = CACHE.with(|cache| cache.borrow_mut().remove(path));
        if let Some((cached_id, store)) = cached {
            if cached_id == id {
                return Ok(PooledStore { path: path.to_path_buf(), id: Some(id), store: Some(store) });
            }
        }
        let migrated = MIGRATED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref().is_some_and(|set| set.contains(&id));
        if migrated {
            if let Ok(store) = Store::open_migrated(path) {
                return Ok(PooledStore { path: path.to_path_buf(), id: Some(id), store: Some(store) });
            }
        }
    }
    let store = Store::open(path)?;
    let id = identity(path);
    if let Some(id) = id {
        MIGRATED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get_or_insert_with(HashSet::new).insert(id);
    }
    Ok(PooledStore { path: path.to_path_buf(), id, store: Some(store) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-pool-{}-{tag}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join("lifescape.db")
    }

    /// The connection is reused on the thread, rebuilt when the file is
    /// replaced, and never kept with a transaction open.
    #[test]
    fn reuses_the_thread_connection_and_follows_the_file() {
        let path = temp_db("reuse");
        {
            let store = open(&path).expect("first open migrates");
            store.begin_batch().expect("batch");
            store.upsert_item(&lifescape_store::Item { item_id: 1, name: "A".to_string(), shop_price: 1, alch_value: 1, }).expect("write");
            store.commit_batch().expect("commit");
        }
        assert!(CACHE.with(|cache| cache.borrow().contains_key(&path)), "returned to the cache");
        {
            let store = open(&path).expect("reused");
            assert_eq!(store.get_item(1).expect("read").map(|item| item.name), Some("A".to_string()));
            store.begin_batch().expect("batch left open");
        }
        assert!(!CACHE.with(|cache| cache.borrow().contains_key(&path)), "a connection with an open transaction is closed");
        std::fs::remove_file(&path).expect("replace the file");
        let store = open(&path).expect("fresh file migrated again");
        assert_eq!(store.get_item(1).expect("read"), None, "a new database, not the old connection");
    }
}
