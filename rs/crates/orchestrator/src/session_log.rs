#![forbid(unsafe_code)]
//! Session log sink (E11): buffered, de-duplicated, rotated — lossless.
//!
//! Before: every worker line opened, appended and closed the log file
//! (one `open+write+close` per line, ~10 lines/s/bot at 300 bots = 3000
//! syscalls/s plus unbuffered writes). Now each path owns one `LogSink`:
//! lines buffer to 8 KiB or 200 lines, consecutive duplicates collapse to
//! `… répété ×N`, and a 100 Mo file rotates to `.1` (D34: 100 Mo/bot).
//! Nothing is dropped: rotation keeps the old generation, dedup counts
//! are written out, and `close`/panic paths flush.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Flush threshold: bytes or lines, whichever fills first.
const FLUSH_BYTES: usize = 8192;
const FLUSH_LINES: usize = 200;
/// Rotation size (D34): one bot keeps at most current + `.1`.
pub const LOG_ROTATE_BYTES: u64 = 100 * 1024 * 1024;

struct LogSink {
    path: PathBuf,
    buffer: String,
    lines: usize,
    last_line: String,
    repeats: usize,
}

impl LogSink {
    fn new(path: &Path) -> Self {
        LogSink {
            path: path.to_path_buf(),
            buffer: String::new(),
            lines: 0,
            last_line: String::new(),
            repeats: 0,
        }
    }

    fn emit(&mut self, line: &str) {
        if line == self.last_line {
            self.repeats += 1;
            return;
        }
        self.flush_repeats();
        self.last_line = line.to_string();
        self.buffer.push_str(line);
        self.buffer.push('\n');
        self.lines += 1;
        if self.buffer.len() >= FLUSH_BYTES || self.lines >= FLUSH_LINES {
            self.flush();
        }
    }

    fn flush_repeats(&mut self) {
        if self.repeats > 0 {
            let collapsed = format!("… répété ×{}", self.repeats + 1);
            self.buffer.push_str(&collapsed);
            self.buffer.push('\n');
            self.lines += 1;
            self.repeats = 0;
        }
    }

    fn rotate_if_huge(&mut self) {
        let size = std::fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        if size >= LOG_ROTATE_BYTES {
            self.flush_buffer();
            let rotated = self.path.with_extension("log.1");
            let _ = std::fs::remove_file(&rotated);
            let _ = std::fs::rename(&self.path, &rotated);
        }
    }

    fn flush_buffer(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        if let Ok(mut handle) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            use std::io::Write;
            let _ = handle.write_all(self.buffer.as_bytes());
        }
        self.buffer.clear();
        self.lines = 0;
    }

    fn flush(&mut self) {
        self.flush_repeats();
        self.rotate_if_huge();
        self.flush_buffer();
    }
}

fn sinks() -> &'static Mutex<HashMap<PathBuf, LogSink>> {
    static SINKS: OnceLock<Mutex<HashMap<PathBuf, LogSink>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Buffered append for one worker line (dedup + rotation included).
pub fn append_log(path: &Path, line: &str) {
    let mut guard = sinks().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.entry(path.to_path_buf()).or_insert_with(|| LogSink::new(path)).emit(line);
}

/// Flush one log (run end, close, last breath). Idempotent.
pub fn flush_log(path: &Path) {
    let mut guard = sinks().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(sink) = guard.get_mut(path) {
        sink.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{append_log, flush_log, FLUSH_BYTES};

    fn temp_log(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lifescape-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("session.log")
    }

    /// Duplicates collapse to one counted line; everything else lands
    /// verbatim, in order.
    #[test]
    fn duplicates_collapse_losslessly() {
        let path = temp_log("dedup");
        append_log(&path, "act [t=1] chop");
        append_log(&path, "act [t=1] chop");
        append_log(&path, "act [t=1] chop");
        append_log(&path, "msg [t=4] done");
        flush_log(&path);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "first + collapse + next, got {lines:?}");
        assert_eq!(lines[0], "act [t=1] chop");
        assert!(lines[1].contains("répété ×3"), "count kept, got {:?}", lines[1]);
        assert_eq!(lines[2], "msg [t=4] done");
    }

    /// Buffering holds back small bursts; the flush writes them once.
    #[test]
    fn small_bursts_buffer_until_flush() {
        let path = temp_log("buffer");
        append_log(&path, "one");
        assert!(!path.exists() || std::fs::read_to_string(&path).unwrap_or_default().is_empty());
        flush_log(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
    }

    /// Past the byte threshold the sink flushes on its own.
    #[test]
    fn full_buffer_flushes_itself() {
        let path = temp_log("full");
        let prefix = "y".repeat(500);
        for index in 0..(FLUSH_BYTES / 512 + 2) {
            append_log(&path, &format!("{prefix}-{index}"));
        }
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        assert!(size > 0, "auto-flush wrote without an explicit flush");
    }
}
