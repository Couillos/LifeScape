#![forbid(unsafe_code)]
//! Daemon downtime memory (user 2026-11-23): a wall-clock wait that a bot
//! suffers by (a market buy's fill deadline, a milestone set aside after
//! a long wait, a blocked bid given back) must not run while the daemon
//! is stopped. 38 buys expired at once after a 7 h stop.
//!
//! `run/downtime.json` holds `last_alive` (rewritten at most once a minute
//! by the supervisor loop, atomic), `started` (this daemon's start) and a
//! bounded list of recent downtime intervals `[from, to)` in unix seconds.
//! At start the interval `[last_alive, start)` joins the list. The wait of
//! a stamp is `now - stamp` minus the downtime overlapping `[stamp, now]`.

use std::sync::Mutex;

use lifescape_json::{Value, object};

/// Most recent downtime intervals kept (a wait spans a few restarts).
pub const KEEP_INTERVALS: usize = 64;
/// Intervals older than this are dropped (no wait lasts that long).
pub const KEEP_SECS: i64 = 14 * 86_400;
/// The liveness stamp is rewritten at most this often.
pub const BEAT_SECS: i64 = 60;

/// Intervals shorter than this are not worth remembering (a clean
/// restart: a few seconds).
const MIN_GAP_SECS: i64 = 1;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Downtime {
    pub last_alive: i64,
    pub started: i64,
    pub intervals: Vec<(i64, i64)>,
}

struct State {
    downtime: Downtime,
    written: i64,
}

fn state() -> &'static Mutex<State> {
    static STATE: Mutex<State> = Mutex::new(State { downtime: Downtime { last_alive: 0, started: 0, intervals: Vec::new() }, written: 0 });
    &STATE
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Seconds of `intervals` inside `[from, to]`.
pub fn overlap_in(intervals: &[(i64, i64)], from: i64, to: i64) -> i64 {
    intervals.iter().map(|&(start, end)| (end.min(to) - start.max(from)).max(0)).sum()
}

/// `now - since` minus the downtime inside `[since, now]` (never negative).
pub fn awake_in(intervals: &[(i64, i64)], since: i64, now: i64) -> i64 {
    ((now - since) - overlap_in(intervals, since, now)).max(0)
}

impl Downtime {
    /// The state after a start at `now`, the daemon last seen alive at
    /// `last_alive` (`None`: no trace at all, nothing to add).
    pub fn restarted(&self, last_alive: Option<i64>, now: i64) -> Downtime {
        let mut intervals = self.intervals.clone();
        if let Some(alive) = last_alive {
            if now - alive >= MIN_GAP_SECS {
                intervals.push((alive, now));
            }
        }
        intervals.retain(|&(_, end)| now - end <= KEEP_SECS);
        intervals.sort_unstable();
        let extra = intervals.len().saturating_sub(KEEP_INTERVALS);
        intervals.drain(..extra);
        Downtime { last_alive: now, started: now, intervals }
    }

    pub fn to_value(&self) -> Value {
        let pairs: Vec<Value> = self.intervals.iter().map(|&(from, to)| Value::Arr(vec![Value::Int(from), Value::Int(to)])).collect();
        object(vec![("last_alive", Value::Int(self.last_alive)), ("started", Value::Int(self.started)), ("intervals", Value::Arr(pairs))])
    }

    pub fn from_value(value: &Value) -> Option<Downtime> {
        let last_alive = value.get("last_alive").as_i64()?;
        let started = value.get("started").as_i64().unwrap_or(last_alive);
        let intervals = value
            .get("intervals")
            .as_arr()
            .map(|rows| rows.iter().filter_map(|row| { let cells = row.as_arr()?; Some((cells.first()?.as_i64()?, cells.get(1)?.as_i64()?)) }).collect())
            .unwrap_or_default();
        Some(Downtime { last_alive, started, intervals })
    }
}

fn persist(downtime: &Downtime) {
    if let Err(error) = crate::paths::write_json(&crate::paths::downtime_path(), &downtime.to_value()) {
        eprintln!("downtime: cannot write {}: {error}", crate::paths::downtime_path().display());
    }
}

/// Daemon start at `now`: reads the liveness file (`fallback_alive` is
/// asked only when the file is missing or unreadable: the newest stamp
/// the daemon persisted before), records `[last_alive, now)` as downtime,
/// installs the state and writes the file. Returns the interval added.
pub fn start(now: i64, fallback_alive: impl FnOnce() -> Option<i64>) -> Option<(i64, i64)> {
    let known = crate::paths::read_json(&crate::paths::downtime_path()).and_then(|value| Downtime::from_value(&value));
    let (base, alive) = match known {
        Some(file) => {
            let alive = file.last_alive;
            (file, Some(alive))
        }
        None => (Downtime::default(), fallback_alive()),
    };
    let next = base.restarted(alive, now.max(alive.unwrap_or(0)));
    let added = next.intervals.last().copied().filter(|&(_, end)| end == now && alive.is_some());
    persist(&next);
    let mut guard = lock();
    guard.downtime = next;
    guard.written = now;
    added
}

/// Supervisor loop beat: rewrites `last_alive` at most once a minute.
pub fn heartbeat(now: i64) {
    let mut guard = lock();
    if now - guard.written < BEAT_SECS {
        return;
    }
    guard.downtime.last_alive = now;
    guard.written = now;
    let snapshot = guard.downtime.clone();
    drop(guard);
    persist(&snapshot);
}

thread_local! {
    /// Per-thread intervals standing in for the installed ones (tests:
    /// one test's downtime never leaks into a test on another thread).
    static OVERRIDE: std::cell::RefCell<Option<Vec<(i64, i64)>>> = const { std::cell::RefCell::new(None) };
}

fn with_intervals<R>(read: impl FnOnce(&[(i64, i64)]) -> R) -> R {
    let local = OVERRIDE.with(|slot| slot.borrow().as_ref().map(|intervals| read_clone(intervals)));
    match local {
        Some(intervals) => read(&intervals),
        None => read(&lock().downtime.intervals),
    }
}

fn read_clone(intervals: &[(i64, i64)]) -> Vec<(i64, i64)> {
    intervals.to_vec()
}

/// Seconds of downtime inside `[from, to]` (0 before `start`, in tests).
pub fn overlap(from: i64, to: i64) -> i64 {
    with_intervals(|intervals| overlap_in(intervals, from, to))
}

/// Wall seconds from `since` to `now` without the daemon's downtime.
pub fn awake_secs(since: i64, now: i64) -> i64 {
    with_intervals(|intervals| awake_in(intervals, since, now))
}

/// Sets the calling thread's intervals (tests; never touches the
/// installed state nor the file).
pub fn set_for_test(intervals: Vec<(i64, i64)>) {
    OVERRIDE.with(|slot| *slot.borrow_mut() = Some(intervals));
}

/// The installed intervals (reports, tests).
pub fn intervals() -> Vec<(i64, i64)> {
    with_intervals(|intervals| intervals.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_counts_only_the_inside() {
        let down = [(100, 200), (300, 400)];
        assert_eq!(overlap_in(&down, 0, 1000), 200);
        assert_eq!(overlap_in(&down, 150, 350), 100);
        assert_eq!(awake_in(&down, 150, 350), 100);
        assert_eq!(awake_in(&down, 450, 500), 50);
        assert_eq!(awake_in(&[], 10, 5), 0);
    }

    #[test]
    fn restart_adds_the_gap_and_bounds_the_list() {
        let first = Downtime::default().restarted(Some(1000), 8000);
        assert_eq!(first.intervals, vec![(1000, 8000)]);
        assert_eq!((first.last_alive, first.started), (8000, 8000));
        let again = first.restarted(Some(9000), 9100);
        assert_eq!(again.intervals, vec![(1000, 8000), (9000, 9100)]);
        let old = again.restarted(None, 8000 + KEEP_SECS + 5000);
        assert_eq!(old.intervals, Vec::<(i64, i64)>::new(), "stale intervals dropped, no trace adds none");
        let mut many = Downtime::default();
        for step in 0..100 {
            many = many.restarted(Some(step * 10), step * 10 + 5);
        }
        assert_eq!(many.intervals.len(), KEEP_INTERVALS);
    }

    #[test]
    fn the_file_round_trips() {
        let down = Downtime { last_alive: 7, started: 5, intervals: vec![(1, 2), (3, 4)] };
        assert_eq!(Downtime::from_value(&down.to_value()), Some(down));
    }

    #[test]
    fn start_reads_the_file_then_falls_back() {
        let path = crate::paths::downtime_path();
        let _ = std::fs::remove_file(&path);
        // No file: the fallback stamp is the last alive.
        let added = start(10_000, || Some(4_000));
        assert_eq!(added, Some((4_000, 10_000)));
        assert_eq!(awake_secs(3_000, 11_000), 8_000 - 6_000);
        // The file now exists: the fallback is not asked.
        heartbeat(10_000 + BEAT_SECS);
        let added = start(20_000, || panic!("file present"));
        assert_eq!(added, Some((10_060, 20_000)));
        assert_eq!(intervals(), vec![(4_000, 10_000), (10_060, 20_000)]);
        let _ = std::fs::remove_file(&path);
    }
}
