#![forbid(unsafe_code)]
//! Bounded blocking calls on reused helper threads (lot 4.5).
//!
//! `std` has no connect timeout for a unix socket, so the daemon's bounded
//! connect runs on a helper thread the caller waits on for at most the
//! timeout. That helper used to be a new `worker-connect` thread per call.
//! Here the helpers are kept: a thread that finished its call waits for the
//! next one (up to `MAX_IDLE` of them stay alive), a call that finds none
//! idle spawns one, and a helper blocked in a wedged connect is simply not
//! idle, so one wedged account never makes another's call wait or time out.
//! Std only: threads, an `mpsc` channel, atomics.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Helper threads kept waiting between calls.
const MAX_IDLE: usize = 8;
/// A connect closure is a few lines: a small stack.
const STACK_BYTES: usize = 256 * 1024;

type Task = Box<dyn FnOnce() -> bool + Send + 'static>;

struct Pool {
    tx: Sender<Task>,
    rx: Arc<Mutex<Receiver<Task>>>,
    /// Helpers blocked in `recv` and not yet claimed by a caller.
    idle: AtomicUsize,
    /// Helpers ever spawned (a gauge for tests and the live hour).
    spawned: AtomicU64,
}

impl Pool {
    fn new() -> Pool {
        let (tx, rx) = channel::<Task>();
        Pool { tx, rx: Arc::new(Mutex::new(rx)), idle: AtomicUsize::new(0), spawned: AtomicU64::new(0) }
    }
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(Pool::new)
}

/// Helpers spawned since the process started.
pub fn spawned() -> u64 {
    pool().spawned.load(Ordering::SeqCst)
}

/// A helper runs its tasks one after the other; a task answers `false`
/// when the helper must end (the pool already keeps `MAX_IDLE` waiting).
fn helper(pool: &'static Pool, first: Option<Task>) {
    if let Some(task) = first {
        if !task() {
            return;
        }
    }
    loop {
        // One helper at a time waits on the channel, the others queue on
        // the lock: all of them are idle.
        let task = pool.rx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
        match task {
            Ok(task) => {
                if !task() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Run `work` on a helper and wait at most `timeout` for its result.
/// `None`: it did not finish in time (the helper stays with it until it
/// returns, then goes back to waiting). A caller reserves an idle helper
/// before sending, so a queued call is always picked up by a free one.
pub fn run_bounded<T: Send + 'static>(timeout: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    run_on(pool(), timeout, work)
}

fn run_on<T: Send + 'static>(pool: &'static Pool, timeout: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (reply, wait) = channel::<T>();
    let task: Task = Box::new(move || {
        let out = work();
        // The helper counts as idle BEFORE the caller can see the answer:
        // its next call (right after this one) finds it, not a new thread.
        let keep = pool.idle.try_update(Ordering::SeqCst, Ordering::SeqCst, |idle| (idle < MAX_IDLE).then_some(idle + 1)).is_ok();
        let _ = reply.send(out);
        keep
    });
    let reserved = pool.idle.try_update(Ordering::SeqCst, Ordering::SeqCst, |idle| idle.checked_sub(1)).is_ok();
    if reserved {
        if pool.tx.send(task).is_err() {
            return None;
        }
    } else {
        pool.spawned.fetch_add(1, Ordering::SeqCst);
        let slot = Arc::new(Mutex::new(Some(task)));
        let carried = slot.clone();
        let spawn = std::thread::Builder::new().name("worker-connect".to_string()).stack_size(STACK_BYTES).spawn(move || {
            let task = carried.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            helper(pool, task);
        });
        if spawn.is_err() {
            return None;
        }
    }
    wait.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls in a row reuse the helper: 200 calls, one thread (a pool of
    /// its own: the process-global one is shared with the other tests).
    #[test]
    fn sequential_calls_reuse_one_helper() {
        let own: &'static Pool = Box::leak(Box::new(Pool::new()));
        for n in 0..200 {
            assert_eq!(run_on(own, Duration::from_secs(5), move || n * 2), Some(n * 2));
        }
        assert_eq!(own.spawned.load(Ordering::SeqCst), 1, "200 sequential calls, one helper");
    }

    /// A wedged call times out, keeps its helper, and the next call is
    /// served at once by another one.
    #[test]
    fn a_wedged_call_times_out_and_does_not_block_the_next() {
        let own: &'static Pool = Box::leak(Box::new(Pool::new()));
        let (release, hold) = channel::<()>();
        let started = std::time::Instant::now();
        let wedged = run_on(own, Duration::from_millis(100), move || {
            let _ = hold.recv_timeout(Duration::from_secs(10));
            7
        });
        assert_eq!(wedged, None, "the wedged call times out");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(run_on(own, Duration::from_secs(5), || "served"), Some("served"), "another helper serves the next call");
        let _ = release.send(());
    }

    /// Concurrent callers all get their own answer (74 accounts at once).
    #[test]
    fn concurrent_calls_all_answer() {
        let own: &'static Pool = Box::leak(Box::new(Pool::new()));
        let callers: Vec<_> = (0..74u64)
            .map(|n| std::thread::spawn(move || run_on(own, Duration::from_secs(10), move || { std::thread::sleep(Duration::from_millis(5)); n })))
            .collect();
        for (n, caller) in callers.into_iter().enumerate() {
            assert_eq!(caller.join().unwrap(), Some(n as u64));
        }
    }
}
