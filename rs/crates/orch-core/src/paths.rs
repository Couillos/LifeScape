//! Filesystem layout and JSON helpers for the daemon state.
//!
//! Everything the daemon writes lives under ``run/`` at the project root:
//! run/users/<account>.json + .bot.json, run/logs/<account>.log,
//! run/orchestrator.sock|pid|log, run/session-<account>.sock|.ready.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lifescape_json::{self as json, Value};

static TRANSACTION: AtomicU64 = AtomicU64::new(0);

/// Project root: ``LIFESCAPE_ROOT`` wins, else the first ancestor of the
/// working dir or the executable that holds an ``orchestration`` dir.
pub fn root() -> PathBuf {
    if let Ok(dir) = env::var("LIFESCAPE_ROOT") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.clone());
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Ok(exe) = env::current_exe() {
        let mut dir = exe.clone();
        for _ in 0..5 {
            if let Some(parent) = dir.parent() {
                candidates.push(parent.to_path_buf());
                dir = parent.to_path_buf();
            }
        }
    }
    for dir in candidates {
        if dir.join("orchestration").is_dir() {
            return dir;
        }
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Test builds (this crate's own tests or the `test-isolation` feature that
/// every dependent crate enables in its dev-dependencies).
const ISOLATED: bool = cfg!(any(test, feature = "test-isolation"));

/// Per-process temp root standing in for `run/` in test builds.
fn isolated_run_dir() -> PathBuf {
    let dir = env::temp_dir().join(format!("lifescape-test-run-{}", std::process::id())).join("run");
    let _ignored = fs::create_dir_all(&dir);
    dir
}

/// Panics when `path` is inside the real `<repo>/run/` (test builds only).
pub fn assert_not_live(path: &Path) {
    lifescape_store::live_guard::assert_not_live(path);
}

/// State directory. Production: `<root>/run`. Test builds: a per-process
/// temp dir, unless a test sets `LIFESCAPE_ROOT` (a copy, never the repo:
/// the tripwire panics on the real run/). Read-only repo assets
/// (gamedata, static, scenarios) keep using `root()`.
pub fn run_dir() -> PathBuf {
    if ISOLATED {
        let explicit = env::var("LIFESCAPE_ROOT").map(|dir| !dir.is_empty()).unwrap_or(false);
        let dir = if explicit { root().join("run") } else { isolated_run_dir() };
        assert_not_live(&dir);
        return dir;
    }
    root().join("run")
}

pub fn users_dir() -> PathBuf {
    run_dir().join("users")
}

pub fn logs_dir() -> PathBuf {
    run_dir().join("logs")
}

pub fn socket_path() -> PathBuf {
    run_dir().join("orchestrator.sock")
}

/// The daemon's SQLite store (`run/lifescape.db`). Test builds get the
/// isolated run dir instead: a test run must never migrate the live
/// file under a running daemon of an older schema (live 2026-11-16: the
/// schema-5 test run locked a schema-4 daemon out of its store).
pub fn store_db() -> PathBuf {
    if ISOLATED {
        let dir = run_dir();
        let _ignored = fs::create_dir_all(&dir);
        return dir.join("lifescape.db");
    }
    run_dir().join("lifescape.db")
}

pub fn pid_path() -> PathBuf {
    run_dir().join("orchestrator.pid")
}

/// Daemon liveness and downtime intervals (`downtime`).
pub fn downtime_path() -> PathBuf {
    run_dir().join("downtime.json")
}

pub fn daemon_log() -> PathBuf {
    run_dir().join("orchestrator.log")
}

/// (merged daemon state, bot heartbeat, log) for one account.
pub fn user_paths(account: &str) -> (PathBuf, PathBuf, PathBuf) {
    (
        users_dir().join(format!("{account}.json")),
        users_dir().join(format!("{account}.bot.json")),
        logs_dir().join(format!("{account}.log")),
    )
}

/// (command socket, ready file) for one persistent session.
pub fn session_paths(account: &str) -> (PathBuf, PathBuf) {
    (
        run_dir().join(format!("session-{account}.sock")),
        run_dir().join(format!("session-{account}.ready")),
    )
}

/// Dedicated push socket next to `run/orchestrator.sock` (workers push
/// heartbeats and task reports to the daemon through it).
pub fn push_socket_path() -> PathBuf {
    run_dir().join("push.sock")
}

/// Life files of the autopilot, one JSON per account.
pub fn lives_dir() -> PathBuf {
    run_dir().join("lives")
}

pub fn life_path(account: &str) -> PathBuf {
    lives_dir().join(format!("{account}.json"))
}

/// Game engine checkout (`LIFESCAPE_ENGINE_DIR`, else the sibling
/// `lostcity/engine` of the project root).
pub fn engine_dir() -> String {
    if let Ok(dir) = env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    let sibling = root().join("../lostcity/engine");
    if sibling.is_dir() {
        return sibling.to_string_lossy().to_string();
    }
    "../lostcity/engine".to_string()
}

pub fn static_dir() -> PathBuf {
    root().join("orchestration").join("static")
}

pub fn ensure_dirs() {
    assert_not_live(&run_dir());
    for dir in [run_dir(), users_dir(), logs_dir()] {
        let _ = fs::create_dir_all(dir);
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    json::parse(&text).ok()
}

/// Atomic write: unique tmp per call (supervisors share account files, a
/// fixed "<path>.tmp" lost the rename race), fsync, then rename.
pub fn write_json(path: &Path, data: &Value) -> io::Result<()> {
    assert_not_live(path);
    let seq = TRANSACTION.fetch_add(1, Ordering::Relaxed);
    let tmp = format!(
        "{}.tmp.{}.{}.{}",
        path.display(),
        std::process::id(),
        format!("{:?}", std::thread::current().id()),
        seq
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&tmp, json::stringify(data))?;
    let handle = fs::File::open(&tmp)?;
    handle.sync_all()?;
    drop(handle);
    fs::rename(&tmp, path)?;
    Ok(())
}
