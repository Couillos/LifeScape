#![forbid(unsafe_code)]
//! Daemon start side of the downtime clock (`orch-core::downtime`): the
//! liveness file is the source; when it does not exist yet (the first
//! start after the feature) the newest stamp the daemon persisted before
//! is the last-alive instant.
//!
//! Fallback chosen: the largest history `ended` over every life file.
//! Every life save is a brain write, so the newest stamp is within a
//! pass of the last activity, and a restored or reverted life set (the
//! operator restoring lives to a past state) carries exactly the state
//! the buys were placed in. `run/lifescape.db` is NOT used: its rows
//! outlive reverted lives (a bugged run's reports stay in the store) and
//! would credit waits that the restored lives never had.

use std::path::Path;

/// Unix seconds now.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Newest history stamp of the life files in `dir` (stamps after `now`
/// are ignored: a bad clock never extends the future).
pub fn newest_life_stamp(dir: &Path, now: i64) -> Option<i64> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: Option<i64> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(value) = crate::paths::read_json(&path) else { continue };
        let Some(life) = lifescape_planner::life_from_value(&value) else { continue };
        let stamp = life.history.iter().map(|history| history.ended).filter(|&ended| ended <= now).max();
        newest = newest.max(stamp);
    }
    newest
}

/// Daemon start: installs the downtime clock (before any brain pass).
pub fn start() {
    let at = now();
    let added = crate::downtime::start(at, || newest_life_stamp(&crate::paths::lives_dir(), at));
    match added {
        Some((from, to)) => crate::logln!("downtime: daemon was down {} s ({from} -> {to}); market waits resume where they stopped", to - from),
        None => crate::logln!("downtime: no earlier trace, nothing to subtract"),
    }
}

#[cfg(test)]
#[path = "brain_downtime_tests.rs"]
mod tests;
