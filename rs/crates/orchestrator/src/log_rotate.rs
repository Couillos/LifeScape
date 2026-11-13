#![forbid(unsafe_code)]
//! Compressed log rotation (user decision 2026-11-13: « rotation zippée »).
//!
//! A log past `ROTATE_BYTES` becomes `<name>.log.<unix secs>`, compressed
//! in the background by the system `gzip` (std only: no compression crate,
//! the binary is spawned) into `<name>.log.<secs>.gz`; the newest
//! `KEEP_GENERATIONS` archives of each log are kept, older ones removed.
//! Bot session logs are renamed (their sink reopens the path on each
//! flush). The daemon log is the daemon's own stdout/stderr (an O_APPEND
//! descriptor): it is copied then truncated while holding the stdout lock,
//! so no `logln!` line is lost. Legacy `.log.1` generations (before this
//! module) are compressed once at daemon start.

use std::path::{Path, PathBuf};

/// Size at which a log rotates (D34: 100 Mo per bot).
pub const ROTATE_BYTES: u64 = 100 * 1024 * 1024;
/// Compressed generations kept per log.
pub const KEEP_GENERATIONS: usize = 5;
/// Seconds between two size checks of the daemon log.
const DAEMON_CHECK_SECS: u64 = 60;

/// Typed rotation failure (logged by the callers, never fatal).
#[derive(Debug)]
pub enum RotateError {
    Io { path: PathBuf, error: String },
    Gzip { path: PathBuf, detail: String },
}

impl std::fmt::Display for RotateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RotateError::Io { path, error } => write!(formatter, "log rotate: {}: {error}", path.display()),
            RotateError::Gzip { path, detail } => write!(formatter, "log rotate: gzip {}: {detail}", path.display()),
        }
    }
}

fn io_error(path: &Path, error: std::io::Error) -> RotateError {
    RotateError::Io { path: path.to_path_buf(), error: error.to_string() }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs()).unwrap_or(0)
}

/// `<file>.<secs>` beside `path`; a clash (two rotations in one second)
/// takes the next free second.
fn generation_path(path: &Path, secs: u64) -> PathBuf {
    let base = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let mut stamp = secs;
    loop {
        let candidate = path.with_file_name(format!("{base}.{stamp}"));
        let archive = path.with_file_name(format!("{base}.{stamp}.gz"));
        if !candidate.exists() && !archive.exists() {
            return candidate;
        }
        stamp += 1;
    }
}

/// Compresses `path` into `path.gz` (gzip removes the source), then
/// prunes the archives of `log` beyond `KEEP_GENERATIONS`.
pub fn compress_and_prune(path: &Path, log: &Path) -> Result<(), RotateError> {
    let output = std::process::Command::new("gzip")
        .arg("-f")
        .arg(path)
        .output()
        .map_err(|error| RotateError::Gzip { path: path.to_path_buf(), detail: error.to_string() })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(RotateError::Gzip { path: path.to_path_buf(), detail });
    }
    prune(log)
}

/// Archives of `log` (`<file>.<secs>.gz`), oldest first.
pub fn archives_of(log: &Path) -> Vec<PathBuf> {
    let Some(dir) = log.parent() else { return Vec::new() };
    let base = log.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    let prefix = format!("{base}.");
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let stamp = name.strip_prefix(&prefix)?.strip_suffix(".gz")?.parse::<u64>().ok()?;
                    Some((stamp, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

fn prune(log: &Path) -> Result<(), RotateError> {
    let archives = archives_of(log);
    let excess = archives.len().saturating_sub(KEEP_GENERATIONS);
    for old in archives.into_iter().take(excess) {
        std::fs::remove_file(&old).map_err(|error| io_error(&old, error))?;
    }
    Ok(())
}

fn compress_in_background(generation: PathBuf, log: PathBuf) {
    let shown = log.display().to_string();
    let spawned = std::thread::Builder::new().name("log-gzip".to_string()).spawn(move || {
        if let Err(error) = compress_and_prune(&generation, &log) {
            crate::logln!("{error}");
        }
    });
    if let Err(error) = spawned {
        crate::logln!("log rotate: no gzip thread for {shown}: {error}");
    }
}

/// Renames `path` to a fresh generation and compresses it in the
/// background. For files reopened by path on each write (session logs).
pub fn rotate_renamed(path: &Path) -> Result<(), RotateError> {
    let generation = generation_path(path, unix_secs());
    std::fs::rename(path, &generation).map_err(|error| io_error(path, error))?;
    compress_in_background(generation, path.to_path_buf());
    Ok(())
}

/// Copies `path` to a fresh generation then truncates it, for a file held
/// open in append mode (the daemon's stdout). The caller holds whatever
/// lock keeps writers out between the copy and the truncation.
pub fn copy_truncate(path: &Path) -> Result<PathBuf, RotateError> {
    let generation = generation_path(path, unix_secs());
    std::fs::copy(path, &generation).map_err(|error| io_error(path, error))?;
    let handle = std::fs::OpenOptions::new().write(true).open(path).map_err(|error| io_error(path, error))?;
    handle.set_len(0).map_err(|error| io_error(path, error))?;
    Ok(generation)
}

fn rotate_daemon_log(path: &Path) -> Result<(), RotateError> {
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size < ROTATE_BYTES {
        return Ok(());
    }
    // `println!` takes this lock for every `logln!` line: holding it makes
    // the copy + truncation atomic for the daemon's own log lines.
    let generation = {
        let _writers = std::io::stdout().lock();
        copy_truncate(path)?
    };
    compress_in_background(generation, path.to_path_buf());
    Ok(())
}

/// Legacy `<file>.1` generations of `dir` (pre-rotation module): renamed
/// to `<file>.<mtime>` and compressed, one after the other.
pub fn migrate_legacy(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut migrated = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(base) = name.strip_suffix(".1") else { continue };
        let log = path.with_file_name(base);
        let mtime = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|span| span.as_secs())
            .unwrap_or_else(unix_secs);
        let generation = generation_path(&log, mtime);
        if let Err(error) = std::fs::rename(&path, &generation) {
            crate::logln!("{}", io_error(&path, error));
            continue;
        }
        match compress_and_prune(&generation, &log) {
            Ok(()) => migrated += 1,
            Err(error) => crate::logln!("{error}"),
        }
    }
    migrated
}

/// Daemon start: compresses legacy generations of the session logs, then
/// checks the daemon log size every minute (background thread).
pub fn start(logs_dir: PathBuf, daemon_log: PathBuf) {
    let spawned = std::thread::Builder::new().name("log-rotate".to_string()).spawn(move || {
        let started = std::time::Instant::now();
        let migrated = migrate_legacy(&logs_dir);
        if migrated > 0 {
            crate::logln!("log rotate: {migrated} legacy .1 log(s) compressed in {:.0}s", started.elapsed().as_secs_f64());
        }
        loop {
            if let Err(error) = rotate_daemon_log(&daemon_log) {
                crate::logln!("{error}");
            }
            // Housekeeping cadence, not a game wait (rule 5 concerns waits
            // on the game; this thread never touches a bot).
            std::thread::sleep(std::time::Duration::from_secs(DAEMON_CHECK_SECS));
        }
    });
    if let Err(error) = spawned {
        crate::logln!("log rotate: thread not started: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-rotate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn gunzip(path: &Path) -> String {
        let output = std::process::Command::new("gzip").arg("-dc").arg(path).output().expect("gzip -dc");
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    #[test]
    fn compressed_generations_keep_the_newest_five() {
        let dir = temp_dir("keep");
        let log = dir.join("bot.log");
        for stamp in 1..=7u64 {
            let generation = log.with_file_name(format!("bot.log.{stamp}"));
            std::fs::write(&generation, format!("generation {stamp}\n")).expect("write");
            compress_and_prune(&generation, &log).expect("compress");
        }
        let archives = archives_of(&log);
        assert_eq!(archives.len(), KEEP_GENERATIONS, "{archives:?}");
        assert!(archives[0].ends_with("bot.log.3.gz"), "oldest kept is 3: {archives:?}");
        assert_eq!(gunzip(&archives[4]), "generation 7\n");
        assert!(!log.with_file_name("bot.log.7").exists(), "gzip replaced the plain file");
    }

    #[test]
    fn copy_truncate_keeps_every_byte_and_empties_the_live_file() {
        let dir = temp_dir("copy");
        let log = dir.join("orchestrator.log");
        std::fs::write(&log, "line one\nline two\n").expect("write");
        let mut writer = std::fs::OpenOptions::new().append(true).open(&log).expect("append handle");
        let generation = copy_truncate(&log).expect("rotate");
        use std::io::Write;
        writer.write_all(b"after\n").expect("append after truncate");
        assert_eq!(std::fs::read_to_string(&generation).expect("generation"), "line one\nline two\n");
        assert_eq!(std::fs::read_to_string(&log).expect("live"), "after\n", "an O_APPEND writer lands at the new start");
    }

    #[test]
    fn legacy_dot_one_logs_are_compressed_once() {
        let dir = temp_dir("legacy");
        std::fs::write(dir.join("t01.log.1"), "old t01\n").expect("write");
        std::fs::write(dir.join("t01.log"), "live t01\n").expect("write");
        assert_eq!(migrate_legacy(&dir), 1);
        assert!(!dir.join("t01.log.1").exists());
        let archives = archives_of(&dir.join("t01.log"));
        assert_eq!(archives.len(), 1);
        assert_eq!(gunzip(&archives[0]), "old t01\n");
        assert_eq!(std::fs::read_to_string(dir.join("t01.log")).expect("live"), "live t01\n");
        assert_eq!(migrate_legacy(&dir), 0, "idempotent");
    }
}
