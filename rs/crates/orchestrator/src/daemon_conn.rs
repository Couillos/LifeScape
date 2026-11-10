#![forbid(unsafe_code)]
//! Worker connections (split from daemon.rs, same behavior): bounded
//! unix connect, the JSON round-trip, live-binary resolution and log tail.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use lifescape_json::{self as json, Value, object};
use crate::daemon::Orchestrator;
use crate::paths;

/// Bounded unix connect: the blocking `connect` runs on a helper
/// thread and the caller waits at most `timeout`. A wedged listener
/// (backlog full, nobody accepting) can no longer park the caller
/// forever; the helper thread is reaped when the worker dies (the
/// caller kills it on timeout, which unblocks the connect).
fn connect_worker(
    sock_path: &std::path::Path,
    timeout: Duration,
) -> Result<UnixStream, String> {
    let path = sock_path.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel::<Result<UnixStream, String>>();
    if std::thread::Builder::new()
        .name("worker-connect".to_string())
        .spawn(move || {
            let _ = sender.send(
                UnixStream::connect(&path).map_err(|error| format!("session unreachable: {error}")),
            );
        })
        .is_err()
    {
        return Err("connect spawn failed".to_string());
    }
    receiver
        .recv_timeout(timeout)
        .map_err(|_| "connect timed out (wedged session)".to_string())?
}

impl Orchestrator {
    /// One JSON round-trip to the session worker. The connect itself
    /// is bounded (helper thread + timeout, reaped on worker death):
    /// a socket nobody accepts never parks the supervisor, the brain,
    /// or a CLI command. A timed-out connect kills the worker on the
    /// spot — it cannot serve anyone — and clean recovery follows.
    pub(crate) fn send_worker(&self, account: &str, request: &Value, timeout: Duration) -> Result<Value, String> {
        let (sock_path, _ready) = paths::session_paths(account);
        let stream = match connect_worker(&sock_path, timeout) {
            Ok(stream) => stream,
            Err(_) => {
                println!("daemon: connect to {account} wedged, killing the worker");
                self.kill_process(account);
                let _ = std::fs::remove_file(&sock_path);
                return Err(format!("session unreachable for {account}"));
            }
        };
        stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let mut buf = json::stringify(request);
        buf.push('\n');
        (&stream).write_all(buf.as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                format!("worker for {account} did not answer in time (busy script or wedged session)")
            } else {
                error.to_string()
            }
        })?;
        if line.trim().is_empty() {
            return Ok(object(vec![]));
        }
        json::parse(&line).map_err(|e| e.to_string())
    }
}
/// Live worker binary behind a `current_exe()` path: after `cargo build`
/// replaces the file, "/proc/self/exe" reads "... (deleted)", so strip
/// that marker. `None` means the path is really gone (no guessing).
pub(crate) fn live_binary(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let rendered = exe.to_string_lossy();
    let live = rendered.strip_suffix(" (deleted)").unwrap_or(&rendered);
    let candidate = std::path::PathBuf::from(live);
    candidate.is_file().then(|| candidate)
}

/// Packet-trace line from `connection.rs` pktlog (`C>S ...` / `S>C ...`,
/// stderr, off unless the file flag or env enables it). Spectate and
/// `logs` drop these; the session log file keeps them for debug.
pub(crate) fn is_pktlog_line(line: &str) -> bool {
    line.starts_with("C>S ") || line.starts_with("S>C ")
}

/// Last lines of a (possibly huge) log without reading it whole.
pub(crate) fn tail_lines(path: &std::path::Path, lines: usize) -> Vec<String> {
    const WINDOW: u64 = 131072;
    use std::io::{Read, Seek};
    let mut handle = match std::fs::File::open(path) {
        Ok(handle) => handle,
        Err(_) => return Vec::new(),
    };
    let size = handle.seek(std::io::SeekFrom::End(0)).unwrap_or(0);
    let offset = size.saturating_sub(std::cmp::max(WINDOW, lines as u64 * 2048));
    if handle.seek(std::io::SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut data = Vec::new();
    if handle.read_to_end(&mut data).is_err() {
        return Vec::new();
    }
    let mut found: Vec<String> = String::from_utf8_lossy(&data).split('\n').map(str::to_string).collect();
    if offset > 0 && !found.is_empty() {
        found.remove(0);
    }
    if lines == 0 {
        return Vec::new();
    }
    // A trailing newline leaves a final empty element: drop it.
    if found.last().is_some_and(|line| line.is_empty()) {
        found.pop();
    }
    if found.len() > lines {
        found[found.len() - lines..].to_vec()
    } else {
        found
    }
}

#[cfg(test)]
mod tests {
    use super::{is_pktlog_line, live_binary};

    #[test]
    fn live_binary_strips_deleted_marker() {
        let dir = std::env::temp_dir().join(format!("lifescape-spawn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("orchestrator");
        std::fs::write(&real, b"fake").unwrap();
        assert_eq!(live_binary(&real), Some(real.clone()));
        let deleted = std::path::PathBuf::from(format!("{} (deleted)", real.display()));
        assert_eq!(live_binary(&deleted), Some(real.clone()));
        assert_eq!(live_binary(&dir.join("absent")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spectate_drops_pktlog_keeps_game() {
        assert!(is_pktlog_line("C>S OPNPC2 id=233 len=2 head=0316 w=1.0 pid=1"));
        assert!(is_pktlog_line("S>C NPC_INFO op=197 len=12 head=1b29 w=1.0 pid=1"));
        assert!(is_pktlog_line("C>S NO_TIMEOUT id=0 len=0 head= w=1.0 pid=1"));
        assert!(!is_pktlog_line("act [t=4264 Lumbridge] op_npc2 goblin slot=790"));
        assert!(!is_pktlog_line("msg [t=3818 Lumbridge] I can't reach that!"));
        assert!(!is_pktlog_line("=== session run t01 woodcut ==="));
        assert!(!is_pktlog_line(""));
        // A line merely mentioning the marker mid-text is game content.
        assert!(!is_pktlog_line("say C>S hello"));
    }
}
