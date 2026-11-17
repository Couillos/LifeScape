//! Quarantine re-test at each deploy (user decision D1, 2026-11-17).
//!
//! A quarantine used to last until an operator ran `check-method` by hand
//! (`tan:cow_hide` stayed quarantined two days after its fix, 5 237 error
//! events). Now, once `restore_jobs` is done after a daemon start (every
//! deploy restarts the daemon), the daemon runs `check-method admin
//! <method> --if-idle` itself for each quarantined method, one at a time:
//! a live check that works lifts the quarantine (`method_health::
//! quarantined` reads `run/method_checks.jsonl`), a failed one leaves it.
//!
//! Cost bounded: at most `MAX_RETESTS` methods per deploy (oldest
//! quarantine first), none already checked against this build (a check
//! newer than the binary proves the current code already), each check
//! capped by `check-method` itself (900 s wall). A busy admin (operator
//! script running) is never disturbed: the check exits 3, logged, skipped.
//! Every start and verdict is one daemon log line; the check's own output
//! lands in `run/logs/method_retest.log`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

/// Staff account the re-tests run on (AGENTS.md rule 11/13: cheats stay
/// on staff).
pub const RETEST_ACCOUNT: &str = "admin";
/// Methods re-tested per deploy at most.
pub const MAX_RETESTS: usize = 8;
/// Actions per re-test (`check-method` count).
pub const RETEST_COUNT: i64 = 5;
/// Seconds after `restore_jobs` before the first re-test (the fleet's
/// workers settle their first launches).
const SETTLE_SECS: u64 = 60;

/// Methods to re-test now: quarantined (`at` > 0), without a check at or
/// after `build_at` (the running binary's build time), oldest quarantine
/// first, at most `MAX_RETESTS`. Pure.
pub fn due(quarantined: &BTreeMap<String, i64>, checks: &[(String, bool, i64)], build_at: i64) -> Vec<String> {
    let mut rows: Vec<(&String, i64)> = quarantined
        .iter()
        .filter(|(_, at)| **at > 0)
        .filter(|(method, _)| !checks.iter().any(|(checked, _, at)| checked == *method && *at >= build_at))
        .map(|(method, at)| (method, *at))
        .collect();
    rows.sort_by_key(|(method, at)| (*at, (*method).clone()));
    rows.into_iter().take(MAX_RETESTS).map(|(method, _)| method.clone()).collect()
}

fn build_time(exe: &std::path::Path) -> i64 {
    std::fs::metadata(exe)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|stamp| stamp.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|span| span.as_secs() as i64)
        .unwrap_or(0)
}

/// Starts the re-test pass in the background (called once
/// `restore_jobs` is done).
pub fn start() {
    let spawned = std::thread::Builder::new().name("quarantine-retest".to_string()).spawn(run);
    if let Err(error) = spawned {
        crate::logln!("quarantine retest: thread failed to start: {error}");
    }
}

fn run() {
    let Ok(current) = std::env::current_exe() else {
        crate::logln!("quarantine retest: no current binary, skipped");
        return;
    };
    let Some(exe) = crate::daemon_conn::live_binary(&current) else {
        crate::logln!("quarantine retest: binary gone at {}, skipped (restart the daemon off the fresh build)", current.display());
        return;
    };
    std::thread::sleep(std::time::Duration::from_secs(SETTLE_SECS));
    let methods = due(&crate::method_health::quarantine_stamps(), &crate::method_health::live_checks(), build_time(&exe));
    if methods.is_empty() {
        crate::logln!("quarantine retest: nothing to re-test on this build");
        return;
    }
    crate::logln!("quarantine retest: {} method(s) on {RETEST_ACCOUNT}: {}", methods.len(), methods.join(", "));
    let log_path = crate::paths::logs_dir().join("method_retest.log");
    for method in methods {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path);
        let mut command = std::process::Command::new(&exe);
        command
            .arg("check-method")
            .arg(RETEST_ACCOUNT)
            .arg(&method)
            .arg(RETEST_COUNT.to_string())
            .arg(crate::method_check::IF_IDLE_FLAG)
            .current_dir(crate::paths::root())
            .stdin(std::process::Stdio::null());
        if let Ok(log) = log {
            if let Ok(copy) = log.try_clone() {
                command.stdout(copy);
            }
            command.stderr(log);
        }
        let started = std::time::Instant::now();
        let status = command.status();
        let wall = started.elapsed().as_secs();
        let lifted = !crate::method_health::quarantined().iter().any(|sick| *sick == method);
        match status.map(|code| code.code()) {
            Ok(Some(3)) => crate::logln!("quarantine retest: {method} skipped, {RETEST_ACCOUNT} busy with an operator script"),
            Ok(code) => crate::logln!(
                "quarantine retest: {method} {} after {wall}s (exit {code:?}){}",
                if lifted { "LIFTED" } else { "still quarantined" },
                if lifted { "" } else { "; see run/logs/method_retest.log" }
            ),
            Err(error) => crate::logln!("quarantine retest: {method} could not start: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{due, MAX_RETESTS};
    use std::collections::BTreeMap;

    /// D1: every quarantined method is re-tested once per build, oldest
    /// first, bounded; a check newer than the build skips it.
    #[test]
    fn each_quarantine_is_retested_once_per_build() {
        let mut table = BTreeMap::new();
        table.insert("tan:cow_hide".to_string(), 100);
        table.insert("fight:melee/chickens".to_string(), 300);
        table.insert("farm:coal".to_string(), 0);
        let none: Vec<(String, bool, i64)> = Vec::new();
        assert_eq!(due(&table, &none, 1000), vec!["tan:cow_hide".to_string(), "fight:melee/chickens".to_string()], "open methods never");
        let checked = vec![("tan:cow_hide".to_string(), false, 1200)];
        assert_eq!(due(&table, &checked, 1000), vec!["fight:melee/chickens".to_string()], "already checked on this build");
        assert_eq!(due(&table, &checked, 1500).len(), 2, "a newer build re-tests it");
        for index in 0..20 {
            table.insert(format!("m{index}"), 500 + index);
        }
        assert_eq!(due(&table, &none, 1000).len(), MAX_RETESTS);
    }
}
