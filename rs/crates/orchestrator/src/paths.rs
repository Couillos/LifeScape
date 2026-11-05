//! Filesystem layout and JSON helpers, mirroring orchestration/paths.py.
//!
//! Everything the daemon writes lives under ``run/`` at the project root:
//! run/users/<account>.json + .bot.json, run/logs/<account>.log,
//! run/orchestrator.sock|pid|log, run/session-<account>.sock|.ready.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::json::{self, Value};

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

pub fn run_dir() -> PathBuf {
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

pub fn pid_path() -> PathBuf {
    run_dir().join("orchestrator.pid")
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

pub fn static_dir() -> PathBuf {
    root().join("orchestration").join("static")
}

pub fn ensure_dirs() {
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
