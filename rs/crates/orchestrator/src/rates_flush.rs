#![forbid(unsafe_code)]
//! Debounced persistence of `run/rates-snapshot.json` (VPS 2026-11-14).
//!
//! The snapshot (~790 Ko) used to be rewritten in full on every report
//! ingest (~20/min live): ~15 Mo/min of disk writes and ~25 ms of JSON
//! encoding per report on the brain thread. Now an ingest only marks the
//! snapshot dirty; the `rates-flush` thread writes it at most once per
//! `FLUSH_PERIOD` when dirty, and the daemon's graceful exit (`shutdown`
//! command, end of `daemon_main`) flushes whatever is left.
//!
//! Loss window: a crash or a SIGTERM/SIGKILL loses at most the ingests
//! of the last `FLUSH_PERIOD` from the snapshot (std only and no
//! `unsafe`: a SIGTERM handler cannot be installed). The raw rows are in
//! the store already (`task_reports`), and the file is reloaded at the
//! next start exactly as before. The write stays atomic (tmp + rename).

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// At most one snapshot write per period while ingests keep coming.
pub const FLUSH_PERIOD: Duration = Duration::from_secs(60);

/// Dirty flag + write accounting, independent of the clock and the disk
/// (the flusher thread and the tests drive it).
#[derive(Default, Debug)]
pub struct Debounce {
    dirty: bool,
    /// Snapshot files written and their total bytes (gauge, tests).
    pub writes: u64,
    pub bytes: u64,
}

impl Debounce {
    /// One ingest changed the in-memory snapshot.
    pub fn mark(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Writes once when dirty (`write` returns the bytes written); a
    /// failed write stays dirty so the next period retries, loud.
    pub fn flush(&mut self, write: &mut dyn FnMut() -> Result<usize, String>) -> Result<bool, String> {
        if !self.dirty {
            return Ok(false);
        }
        let written = write()?;
        self.dirty = false;
        self.writes += 1;
        self.bytes += written as u64;
        Ok(true)
    }
}

fn state() -> &'static Mutex<Debounce> {
    static STATE: OnceLock<Mutex<Debounce>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(Debounce::default()))
}

/// Marks the snapshot dirty and makes sure the flusher thread runs.
pub fn mark_dirty() {
    state().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).mark();
    static FLUSHER: OnceLock<()> = OnceLock::new();
    FLUSHER.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("rates-flush".to_string()).spawn(|| loop {
            // Disk debounce timer, not a game wait (rule 5 covers game waits).
            std::thread::sleep(FLUSH_PERIOD);
            flush_now();
        });
        if let Err(error) = spawned {
            eprintln!("rates-flush thread spawn failed ({error}): snapshot written at shutdown only");
        }
    });
}

/// Writes the snapshot now when dirty (flusher period, daemon shutdown).
/// The state lock serializes concurrent flushes: never two writers on the
/// same tmp file. A failure is logged and retried at the next period.
pub fn flush_now() {
    let mut guard = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(error) = guard.flush(&mut crate::rates_service::write_snapshot_file) {
        eprintln!("rates snapshot flush failed (retried next period): {error}");
    }
}

/// (writes, bytes) since the daemon started.
pub fn written() -> (u64, u64) {
    let guard = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    (guard.writes, guard.bytes)
}

#[cfg(test)]
mod tests {
    use super::Debounce;

    /// One minute of live ingests (one report every ~3 s): the old path
    /// wrote the 790 Ko file 20 times, the debounce once per period.
    #[test]
    fn one_minute_of_ingests_writes_once() {
        const SNAPSHOT_BYTES: usize = 790 * 1024;
        let mut debounce = Debounce::default();
        let mut before_bytes = 0u64;
        for _report in 0..20 {
            debounce.mark();
            before_bytes += SNAPSHOT_BYTES as u64;
        }
        let mut write = || Ok(SNAPSHOT_BYTES);
        assert_eq!(debounce.flush(&mut write), Ok(true), "dirty: the period writes once");
        assert_eq!(debounce.flush(&mut write), Ok(false), "clean: nothing to write");
        assert_eq!((debounce.writes, debounce.bytes), (1, SNAPSHOT_BYTES as u64));
        assert_eq!(before_bytes / debounce.bytes, 20, "20x fewer bytes per minute");
    }

    #[test]
    fn failed_write_stays_dirty_and_retries() {
        let mut debounce = Debounce::default();
        debounce.mark();
        let mut failing = || Err("disk full".to_string());
        assert!(debounce.flush(&mut failing).is_err());
        assert!(debounce.is_dirty(), "a failed write is retried next period");
        let mut write = || Ok(10);
        assert_eq!(debounce.flush(&mut write), Ok(true));
        assert!(!debounce.is_dirty());
    }
}
