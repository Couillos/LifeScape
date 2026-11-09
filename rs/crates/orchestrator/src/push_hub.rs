#![forbid(unsafe_code)]
//! Worker→daemon push hub (E2): ONE persistent write-only connection per
//! worker, ONE reader thread daemon-side, zero thread per message.
//!
//! The old path opened a Unix connection (and the daemon spawned a thread)
//! per push frame; with snapshots, events, reports and 35 nav polls per
//! plan that was hundreds of connects+threads per second at 300 bots.
//! Now the worker holds a single `push.sock` stream for its whole life
//! (fire-and-forget frames, no reply read) and the daemon scans every
//! persistent stream from one `push-read` thread with non-blocking reads.
//! The CLI socket keeps its 1 req/1 reply shape untouched.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, Value};

/// Dedicated push socket next to `run/orchestrator.sock`.
pub fn socket_path() -> PathBuf {
    crate::paths::run_dir().join("push.sock")
}

/// Split complete newline frames out of a pending buffer. Returns the
/// frames and the bytes consumed; a 1 Mo runaway line resets the buffer
/// instead of growing it forever.
pub fn extract_frames(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut frames = Vec::new();
    let mut consumed = 0usize;
    // Manual scan (no helper split: the buffer stays for the tail).
    let mut start = 0usize;
    let mut index = 0usize;
    while index < buffer.len() {
        if buffer[index] == b'\n' {
            let line = String::from_utf8_lossy(&buffer[start..index]).to_string();
            if !line.trim().is_empty() {
                frames.push(line);
            }
            consumed = index + 1;
            start = index + 1;
            if frames.len() >= 1024 {
                break;
            }
        }
        index += 1;
        if buffer.len() - start > 1_048_576 {
            // Runaway line: drop it loudly, keep the hub alive.
            eprintln!("push hub: overlong line dropped");
            consumed = buffer.len();
            frames.clear();
            break;
        }
    }
    if consumed > 0 {
        buffer.drain(..consumed);
    }
    frames
}

struct PushHub {
    streams: Mutex<HashMap<String, UnixStream>>,
    pending: Mutex<HashMap<String, Vec<u8>>>,
}

impl PushHub {
    fn insert(&self, account: String, stream: UnixStream) {
        stream.set_nonblocking(true).ok();
        self.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(account.clone(), stream);
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&account);
    }

    /// One scan over every persistent stream: non-blocking reads, frame
    /// dispatch, dead streams reaped. Runs on the single `push-read`
    /// thread; no thread per message, ever.
    fn scan_once(&self, dispatch: &Arc<dyn Fn(Value) + Send + Sync>) {
        // Snapshot the keys so a slow stream never holds the map lock.
        let accounts: Vec<String> = self
            .streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect();
        let mut dead = Vec::new();
        for account in accounts {
            let mut chunk = [0u8; 8192];
            let outcome = (|| -> std::io::Result<Option<Vec<String>>> {
                let mut streams =
                    self.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(stream) = streams.get_mut(&account) else {
                    return Ok(None);
                };
                match stream.read(&mut chunk) {
                    Ok(0) => Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "eof")),
                    Ok(read) => {
                        drop(streams);
                        let mut pending =
                            self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        let buffer = pending.entry(account.clone()).or_default();
                        buffer.extend_from_slice(&chunk[..read]);
                        Ok(Some(extract_frames(buffer)))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(Some(Vec::new())),
                    Err(error) => Err(error),
                }
            })();
            match outcome {
                Ok(Some(frames)) => {
                    for frame in frames {
                        match json::parse(&frame) {
                            Ok(value) => dispatch(value),
                            Err(error) => eprintln!("push hub {account}: bad frame: {error}"),
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => dead.push(account),
            }
        }
        if !dead.is_empty() {
            let mut streams = self.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut pending = self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for account in dead {
                streams.remove(&account);
                pending.remove(&account);
            }
        }
    }
}

/// Start the hub: one `push-accept` thread plus one `push-read` thread,
/// both named for `top -H`. The first frame on a stream names its
/// account (`{"hello":"push","account":"t01"}`); later frames are the
/// usual push frames. Unknown first lines are dropped loudly.
pub fn start_push_hub(dispatch: Arc<dyn Fn(Value) + Send + Sync>) -> std::io::Result<()> {
    let sock = socket_path();
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "push hub already running"));
        }
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock)?;
    let hub = Arc::new(PushHub {
        streams: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
    });
    let accept_hub = hub.clone();
    std::thread::Builder::new()
        .name("push-accept".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_nonblocking(false);
                let mut line = String::new();
                let hello = (|| -> Option<String> {
                    use std::io::{BufRead, BufReader};
                    let mut reader = BufReader::new(&stream);
                    reader.read_line(&mut line).ok()?;
                    let value = json::parse(&line).ok()?;
                    if value.get("hello").as_str() != Some("push") {
                        return None;
                    }
                    Some(value.get("account").as_str().unwrap_or("").to_string())
                })();
                let Some(account) = hello.filter(|name| !name.is_empty()) else {
                    eprintln!("push hub: rejecting non-hello first line");
                    continue;
                };
                let _ = stream.set_read_timeout(None);
                crate::res_stats::record_connection();
                accept_hub.insert(account, stream);
            }
        })
        .expect("push accept spawns");
    std::thread::Builder::new()
        .name("push-read".to_string())
        .spawn(move || loop {
            hub.scan_once(&dispatch);
            std::thread::sleep(Duration::from_millis(20));
        })
        .expect("push read spawns");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_frames;

    /// Frames split on newlines, blanks skipped, tail kept for later.
    #[test]
    fn frames_split_and_keep_the_tail() {
        let mut buffer = b"{\"a\":1}\n\n{\"b\":2}\n{\"c\":".to_vec();
        let frames = extract_frames(&mut buffer);
        assert_eq!(frames, vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]);
        assert_eq!(buffer, b"{\"c\":", "tail waits for its newline");
        buffer.extend_from_slice(b"3}\n");
        let frames = extract_frames(&mut buffer);
        assert_eq!(frames, vec!["{\"c\":3}".to_string()]);
        assert!(buffer.is_empty());
    }

    /// A runaway line resets instead of growing the hub forever.
    #[test]
    fn runaway_line_resets_loudly() {
        let mut buffer = vec![b'x'; 1_048_577];
        let frames = extract_frames(&mut buffer);
        assert!(frames.is_empty(), "runaway dropped, not dispatched");
        assert!(buffer.is_empty());
    }
}
