//! Quarantine re-test at each deploy (user decision D1, 2026-11-17).
//!
//! A quarantine used to last until an operator ran `check-method` by hand
//! (`tan:cow_hide` stayed quarantined two days after its fix, 5 237 error
//! events). Now, once `restore_jobs` is done after a daemon start (every
//! deploy restarts the daemon), the daemon runs `check-method <staff>
//! <method> --if-idle` itself for each quarantined method: a live check
//! that works lifts the quarantine (`method_health::quarantined` reads
//! `run/method_checks.jsonl`), a failed one leaves it.
//!
//! Tour 2 (P2-10, decision D7): the methods wait in one queue drained in
//! parallel by the two staff test accounts (`admin`, `admin2`). A busy
//! account (operator script running, the check exits 3) puts the method
//! back and takes the queue again later, never "skipped" for good. A
//! check whose harness failed (`setup` verdict: the dispose step, a
//! missing session) is no verdict: the method is queued again, bounded.
//!
//! Cost bounded: at most `MAX_RETESTS` methods per deploy (oldest
//! quarantine first), none already checked against this build (a check
//! newer than the binary proves the current code already), each check
//! capped by `check-method` itself (900 s wall). Every start and verdict
//! is one daemon log line; the check's own output lands in
//! `run/logs/method_retest.log`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Staff accounts the re-tests run on, in parallel (decision D7:
/// `admin`, plus `admin2` as the second test account; AGENTS.md rules
/// 11-13: cheats stay on staff).
pub const RETEST_ACCOUNTS: [&str; 2] = ["admin", "admin2"];
/// Methods re-tested per deploy at most.
pub const MAX_RETESTS: usize = 8;
/// Actions per re-test (`check-method` count).
pub const RETEST_COUNT: i64 = 5;
/// Seconds after `restore_jobs` before the first re-test (the fleet's
/// workers settle their first launches).
const SETTLE_SECS: u64 = 60;
/// Seconds a test account busy with an operator script waits before it
/// takes the queue again (D7: the queue resumes once it is free).
const BUSY_RETRY_SECS: u64 = 300;
/// Harness failures (`setup`) a method may hit before it leaves the
/// queue for this deploy (a broken harness never loops forever).
pub const MAX_SETUP_RETRIES: u32 = 2;

/// What one re-test run says about its method (pure, tour 2 P2-10).
#[derive(Debug, PartialEq, Eq)]
pub enum RetestStep {
    /// The account was busy (exit 3): the method goes back in the queue.
    Busy,
    /// The harness failed before judging (`setup`): to redo, never
    /// "still quarantined".
    Setup,
    /// Judged: lifted or still quarantined.
    Judged { lifted: bool },
}

/// Reads one re-test from its exit code and the latest check verdict.
pub fn step_of(exit_code: Option<i32>, verdict: Option<&str>, lifted: bool) -> RetestStep {
    if exit_code == Some(3) {
        return RetestStep::Busy;
    }
    if !lifted && verdict == Some(crate::method_health::SETUP_VERDICT) {
        return RetestStep::Setup;
    }
    RetestStep::Judged { lifted }
}

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

type RetestQueue = Mutex<VecDeque<(String, u32)>>;

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
    crate::logln!("quarantine retest: {} method(s) queued on {}: {}", methods.len(), RETEST_ACCOUNTS.join(" + "), methods.join(", "));
    let queue: Arc<RetestQueue> = Arc::new(Mutex::new(methods.into_iter().map(|method| (method, 0u32)).collect()));
    let mut workers = Vec::new();
    for account in RETEST_ACCOUNTS {
        let (queue, exe) = (queue.clone(), exe.clone());
        match std::thread::Builder::new().name(format!("quarantine-retest-{account}")).spawn(move || drain(account, &exe, &queue)) {
            Ok(worker) => workers.push(worker),
            Err(error) => crate::logln!("quarantine retest: {account} worker failed to start: {error}"),
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
    crate::logln!("quarantine retest: queue drained");
}

/// One test account takes methods off the shared queue until it is
/// empty: busy puts the method back and waits, a harness failure puts it
/// back (bounded), a verdict is logged.
fn drain(account: &str, exe: &std::path::Path, queue: &RetestQueue) {
    loop {
        let next = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).pop_front();
        let Some((method, setups)) = next else { return };
        let started = std::time::Instant::now();
        let Some(code) = retest_once(account, exe, &method) else { continue };
        let wall = started.elapsed().as_secs();
        let lifted = !crate::method_health::quarantined().iter().any(|sick| *sick == method);
        let verdict = crate::method_health::last_verdict(&method);
        match step_of(code, verdict.as_deref(), lifted) {
            RetestStep::Busy => {
                crate::logln!("quarantine retest: {method} waits, {account} busy with an operator script (retry in {BUSY_RETRY_SECS}s)");
                queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push_back((method, setups));
                std::thread::sleep(std::time::Duration::from_secs(BUSY_RETRY_SECS));
            }
            RetestStep::Setup if setups < MAX_SETUP_RETRIES => {
                crate::logln!("quarantine retest: {method} harness failed on {account} (setup, not a verdict), queued again");
                queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push_back((method, setups + 1));
            }
            RetestStep::Setup => crate::logln!(
                "quarantine retest: {method} harness failed {} times (setup), left for the next deploy; see run/logs/method_retest.log",
                setups + 1
            ),
            RetestStep::Judged { lifted } => crate::logln!(
                "quarantine retest: {method} {} on {account} after {wall}s (exit {code:?}){}",
                if lifted { "LIFTED" } else { "still quarantined" },
                if lifted { "" } else { "; see run/logs/method_retest.log" }
            ),
        }
    }
}

/// Runs `check-method <account> <method> <count> --if-idle`, output
/// appended to `run/logs/method_retest.log`. `None` when it could not
/// start (logged).
fn retest_once(account: &str, exe: &std::path::Path, method: &str) -> Option<Option<i32>> {
    let log_path = crate::paths::logs_dir().join("method_retest.log");
    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path);
    let mut command = std::process::Command::new(exe);
    command
        .arg("check-method")
        .arg(account)
        .arg(method)
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
    match command.status() {
        Ok(status) => Some(status.code()),
        Err(error) => {
            crate::logln!("quarantine retest: {method} could not start on {account}: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{due, step_of, RetestStep, MAX_RETESTS};
    use std::collections::BTreeMap;

    /// Tour 2 P2-10 red test: a busy account puts the method back in the
    /// queue (D7), a harness failure is no verdict, a judged check is.
    #[test]
    fn busy_and_setup_are_no_verdict() {
        assert_eq!(step_of(Some(3), None, false), RetestStep::Busy);
        assert_eq!(step_of(Some(1), Some("setup"), false), RetestStep::Setup, "check_dispose leftovers: harness, not method");
        assert_eq!(step_of(Some(1), Some("no_output"), false), RetestStep::Judged { lifted: false });
        assert_eq!(step_of(Some(0), Some("ok"), true), RetestStep::Judged { lifted: true });
    }

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
