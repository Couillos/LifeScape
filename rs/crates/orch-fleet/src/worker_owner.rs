#![forbid(unsafe_code)]
//! Session worker ownership: one live worker per account, ever.
//!
//! Live 2026-11-13 (ryan_park): an operator `open` and a market-absent
//! `open` raced (20:17:07 / 20:17:10). The second spawn deleted the first
//! worker's socket and replaced its `procs` entry, dropping its `Child`
//! without killing it: an untracked worker, logged in, that no `close`
//! could reach, and every later login refused "already logged in".
//!
//! Three guards live here, all std-only:
//! - `LifecycleLocks`: one open/close/redeploy per account at a time;
//! - `live_workers`: the truth about running workers, read from `/proc`
//!   (`<exe> session <account>` rooted at our checkout), tracked or not;
//! - `claim_socket`: a worker never unlinks a socket a live worker serves.

use std::collections::HashSet;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

/// SIGTERM grace before SIGKILL, wall seconds (process-side bound, no
/// game connection involved).
pub const TERM_GRACE_SECS: f64 = 8.0;

/// One lifecycle operation (open, close, redeploy, watchdog respawn) per
/// account. A second one is refused at once instead of racing the first.
#[derive(Default)]
pub struct LifecycleLocks {
    busy: Mutex<HashSet<String>>,
}

/// Held for the whole operation; dropping it frees the account.
pub struct LifecycleClaim<'locks> {
    locks: &'locks LifecycleLocks,
    account: String,
}

impl LifecycleLocks {
    pub fn try_claim(&self, account: &str) -> Option<LifecycleClaim<'_>> {
        let mut busy = self.busy.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !busy.insert(account.to_string()) {
            return None;
        }
        Some(LifecycleClaim { locks: self, account: account.to_string() })
    }

    pub fn is_busy(&self, account: &str) -> bool {
        self.busy.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).contains(account)
    }
}

impl Drop for LifecycleClaim<'_> {
    fn drop(&mut self) {
        let mut busy = self.locks.busy.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        busy.remove(&self.account);
    }
}

/// `/proc/<pid>/cmdline` of a session worker: `<exe>\0session\0<account>\0`.
pub fn is_session_worker_cmdline(cmdline: &[u8], account: &str) -> bool {
    let mut parts = cmdline.split(|byte| *byte == 0);
    let _exe = parts.next();
    parts.next() == Some(b"session".as_slice()) && parts.next() == Some(account.as_bytes())
}

/// What `open` may do, given the daemon's own record and the processes
/// actually running for the account.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenVerdict {
    /// No worker anywhere: spawn one.
    Spawn,
    /// The daemon tracks a live worker (or one answers its socket while
    /// `restore_jobs` adopts it): nothing to do.
    AlreadyOpen,
    /// A live worker the daemon does not own: never spawn beside it.
    Foreign(u32),
}

pub fn open_verdict(tracked_alive: bool, live_pids: &[u32], answers_ping: bool) -> OpenVerdict {
    if tracked_alive {
        return OpenVerdict::AlreadyOpen;
    }
    match live_pids.first() {
        None => OpenVerdict::Spawn,
        Some(_) if answers_ping => OpenVerdict::AlreadyOpen,
        Some(pid) => OpenVerdict::Foreign(*pid),
    }
}

/// Typed refusal for `OpenVerdict::Foreign`, naming the pid and the cure.
pub fn foreign_worker_error(account: &str, pid: u32) -> String {
    format!("live worker pid {pid} for {account} is not owned by this daemon (`close {account}` reaps it)")
}

/// Pids whose `/proc` entry passes `matches(cmdline, cwd)`; the caller's
/// own pid is never listed. Zombies have an empty cmdline: never listed.
pub fn scan_proc(matches: impl Fn(&[u8], &Path) -> bool) -> Vec<u32> {
    let own = std::process::id();
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if cmdline.is_empty() {
            continue;
        }
        let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) else {
            continue;
        };
        if matches(&cmdline, &cwd) {
            found.push(pid);
        }
    }
    found.sort_unstable();
    found
}

/// Live session workers of `account` started from `root` (their cwd), the
/// daemon's tracking notwithstanding. A worker of another checkout (a
/// worktree's own daemon) is not ours and never listed.
pub fn live_workers_in(account: &str, root: &Path) -> Vec<u32> {
    let wanted: PathBuf = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    scan_proc(|cmdline, cwd| {
        is_session_worker_cmdline(cmdline, account)
            && cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()) == wanted
    })
}

pub fn live_workers(account: &str) -> Vec<u32> {
    live_workers_in(account, &crate::paths::root())
}

/// Alive and not a zombie (`State: Z` once exited, before its reap).
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => !status.lines().any(|line| line.starts_with("State:") && line.contains('Z')),
        Err(_) => false,
    }
}

/// SIGTERM, wait `TERM_GRACE_SECS`, then SIGKILL (std has no signals:
/// `/bin/kill`). Pid 0 is refused: `kill 0` would hit our process group.
pub fn terminate_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).output();
    if wait_gone(&[pid], TERM_GRACE_SECS) {
        return true;
    }
    let _ = Command::new("kill").arg("-KILL").arg(pid.to_string()).output();
    wait_gone(&[pid], 2.0)
}

/// Poll until every pid is gone, at most `secs` wall seconds.
pub fn wait_gone(pids: &[u32], secs: f64) -> bool {
    let deadline = crate::jobs::now_secs() + secs;
    loop {
        if pids.iter().all(|pid| !pid_alive(*pid)) {
            return true;
        }
        if crate::jobs::now_secs() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Someone accepts on this socket (a live worker, busy or not: the
/// kernel queues the connect even if its serve loop is slow).
pub fn socket_has_listener(path: &Path) -> bool {
    path.exists() && UnixStream::connect(path).is_ok()
}

/// Bind the worker socket. A stale file (no listener) is replaced; a
/// socket a live worker serves is never unlinked (typed refusal).
pub fn claim_socket(path: &Path) -> Result<UnixListener, String> {
    if socket_has_listener(path) {
        return Err(format!("socket {} is served by a live worker, refusing to steal it", path.display()));
    }
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path).map_err(|error| format!("bind failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-owner-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The race of 20:17: a second open while the first is in flight is
    /// refused, and the account frees itself when the first one ends.
    #[test]
    fn one_lifecycle_operation_per_account() {
        let locks = LifecycleLocks::default();
        let first = locks.try_claim("ryan_park").expect("first open claims");
        assert!(locks.try_claim("ryan_park").is_none(), "concurrent open refused");
        assert!(locks.is_busy("ryan_park"));
        let other = locks.try_claim("lara_lane");
        assert!(other.is_some(), "other accounts never wait");
        drop(first);
        assert!(!locks.is_busy("ryan_park"));
        assert!(locks.try_claim("ryan_park").is_some(), "freed once the first ends");
    }

    #[test]
    fn worker_cmdline_matches_its_account_only() {
        let worker = b"/home/x/rs/target/debug/orchestrator\0session\0ryan_park\0";
        assert!(is_session_worker_cmdline(worker, "ryan_park"));
        assert!(!is_session_worker_cmdline(worker, "ryan"), "no prefix match");
        assert!(!is_session_worker_cmdline(b"orchestrator\0open\0ryan_park\0", "ryan_park"));
        assert!(!is_session_worker_cmdline(b"orchestrator\0direct\0ryan_park\0woodcut\0", "ryan_park"));
        assert!(!is_session_worker_cmdline(b"", "ryan_park"), "zombie cmdline is empty");
    }

    /// Never a second worker beside a live one, tracked or not.
    #[test]
    fn open_never_spawns_beside_a_live_worker() {
        assert_eq!(open_verdict(false, &[], false), OpenVerdict::Spawn);
        assert_eq!(open_verdict(true, &[15540], false), OpenVerdict::AlreadyOpen);
        assert_eq!(open_verdict(false, &[15540], true), OpenVerdict::AlreadyOpen, "restore adopts it");
        assert_eq!(open_verdict(false, &[15540], false), OpenVerdict::Foreign(15540), "orphan: refuse");
        let error = foreign_worker_error("ryan_park", 15540);
        assert!(error.contains("15540") && error.contains("close ryan_park"), "{error}");
        assert!(!error.starts_with("already open"), "market-absent must not wait on an orphan forever");
    }

    /// The `/proc` scan finds a live process by cmdline + cwd, then not
    /// once it is reaped; `terminate_pid` ends it within the grace.
    #[test]
    fn proc_scan_finds_then_loses_a_terminated_process() {
        let dir = scratch("scan");
        let wanted = dir.canonicalize().unwrap();
        let mut child = Command::new("sleep").arg("317").current_dir(&dir).spawn().unwrap();
        let pid = child.id();
        let matcher = |cmdline: &[u8], cwd: &Path| cmdline == b"sleep\x00317\x00" && cwd == wanted;
        // Between fork and exec the child still shows our cmdline: poll.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while scan_proc(matcher).is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(scan_proc(matcher), vec![pid]);
        assert!(pid_alive(pid));
        let reaper = std::thread::spawn(move || child.wait());
        assert!(terminate_pid(pid), "SIGTERM ends it within the grace");
        let _ = reaper.join();
        assert!(scan_proc(matcher).is_empty(), "gone once reaped");
        assert!(!pid_alive(pid));
        assert!(!terminate_pid(0), "pid 0 is never signalled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Workers of another checkout are not ours.
    #[test]
    fn live_workers_ignore_other_roots() {
        let dir = scratch("roots");
        assert!(live_workers_in("ryan_park", &dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The second worker of 20:17 unlinked the first one's socket: a
    /// served socket is refused, a stale file is replaced.
    #[test]
    fn claim_socket_never_steals_a_live_socket() {
        let dir = scratch("sock");
        let path = dir.join("session-ryan_park.sock");
        assert!(!socket_has_listener(&path));
        let first = claim_socket(&path).expect("free path binds");
        assert!(socket_has_listener(&path));
        let error = claim_socket(&path).expect_err("live socket refused");
        assert!(error.contains("live worker"), "{error}");
        assert!(socket_has_listener(&path), "the live worker keeps its socket");
        drop(first);
        // A sibling test's fork may hold the fd until its exec (CLOEXEC).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while socket_has_listener(&path) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(path.exists() && !socket_has_listener(&path), "stale file, nobody listens");
        let _second = claim_socket(&path).expect("stale socket replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
