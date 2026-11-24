#![forbid(unsafe_code)]
//! Worker→daemon push hub (E2): ONE persistent write-only connection per
//! worker, ONE blocked reader thread per worker daemon-side, zero thread per
//! message.
//!
//! The old path opened a Unix connection (and the daemon spawned a thread)
//! per push frame; with snapshots, events, reports and 35 nav polls per
//! plan that was hundreds of connects+threads per second at 300 bots.
//! Now the worker holds a single `push.sock` stream for its whole life
//! (fire-and-forget frames, no reply read) and the daemon reads each
//! persistent stream from its own `push-read` thread, blocked in `read`
//! (lot 4.1; the 20 ms polling sweep is gone).
//! The CLI socket keeps its 1 req/1 reply shape untouched.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, Value};

/// Dedicated push socket next to `run/orchestrator.sock`.
pub fn socket_path() -> PathBuf {
    crate::paths::push_socket_path()
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

/// Stack of one reader thread: it only reads, splits lines, parses and
/// calls `dispatch` (frames are heap trees), so a small stack is enough.
const READER_STACK_BYTES: usize = 512 * 1024;

/// One persistent stream per account (lot 4.1): the map keeps a clone of
/// each stream (to shut a replaced connection down) and its generation (a
/// reader only reaps its own entry, never its successor's).
struct PushHub {
    streams: Mutex<HashMap<String, (u64, UnixStream)>>,
    generation: AtomicU64,
}

impl PushHub {
    fn new() -> Self {
        PushHub { streams: Mutex::new(HashMap::new()), generation: AtomicU64::new(0) }
    }

    /// Adopt a stream: ONE `push-read` thread blocked in `read` for its
    /// whole life (no polling sweep, no thread per message; the thread
    /// wakes only when bytes arrive and exits at EOF). A second
    /// connection for the same account replaces the first, which is shut
    /// down so its reader ends. Frames of one account are dispatched in
    /// arrival order by its single reader.
    fn attach(self: &Arc<Self>, account: String, stream: UnixStream, dispatch: Arc<dyn Fn(Value, u64) + Send + Sync>) -> std::io::Result<()> {
        stream.set_nonblocking(false).ok();
        let keep = stream.try_clone()?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self
            .streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(account.clone(), (generation, keep));
        if let Some((_, old)) = previous {
            let _ = old.shutdown(std::net::Shutdown::Both);
        }
        let hub = self.clone();
        let spawned = std::thread::Builder::new()
            .name("push-read".to_string())
            .stack_size(READER_STACK_BYTES)
            .spawn(move || {
                hub.read_loop(&account, stream, &dispatch);
                let mut streams = hub.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if streams.get(&account).is_some_and(|(current, _)| *current == generation) {
                    streams.remove(&account);
                }
            });
        match spawned {
            Ok(_) => Ok(()),
            Err(error) => {
                let mut streams = self.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                streams.retain(|_, (current, _)| *current != generation);
                Err(error)
            }
        }
    }

    /// Blocked reads until EOF or error: bytes, frames, dispatch.
    fn read_loop(&self, account: &str, mut stream: UnixStream, dispatch: &Arc<dyn Fn(Value, u64) + Send + Sync>) {
        let mut chunk = [0u8; 8192];
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let read = match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            };
            buffer.extend_from_slice(&chunk[..read]);
            for frame in extract_frames(&mut buffer) {
                // The received line's length is the frame's byte count
                // (the worker writes `stringify` + newline): no
                // re-serialisation of the parsed frame.
                match json::parse(&frame) {
                    Ok(value) => dispatch(value, frame.len() as u64),
                    Err(error) => eprintln!("push hub {account}: bad frame: {error}"),
                }
            }
        }
    }

    /// Accounts with a live reader (tests and gauges).
    #[cfg(test)]
    fn connected(&self) -> Vec<String> {
        let mut names: Vec<String> = self.streams.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).keys().cloned().collect();
        names.sort();
        names
    }
}

/// Start the hub (`dispatch` gets each parsed frame with the byte length of
/// the line it came from): one `push-accept` thread plus one blocked
/// `push-read` thread per connected worker, all named for `top -H`. The first
/// frame on a stream names its account (`{"hello":"push","account":"t01"}`);
/// later frames are the usual push frames. Unknown first lines are dropped
/// loudly.
pub fn start_push_hub(dispatch: Arc<dyn Fn(Value, u64) + Send + Sync>) -> std::io::Result<()> {
    let sock = socket_path();
    if sock.exists() {
        if UnixStream::connect(&sock).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "push hub already running"));
        }
        let _ = std::fs::remove_file(&sock);
    }
    let listener = UnixListener::bind(&sock)?;
    let hub = Arc::new(PushHub::new());
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
                    // Byte-wise through the stream itself: a BufReader
                    // would swallow frames sent right after the hello.
                    let mut reader = BufReader::with_capacity(1, &stream);
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
                if let Err(error) = hub.attach(account.clone(), stream, dispatch.clone()) {
                    eprintln!("push hub {account}: reader not started: {error}");
                }
            }
        })
        .expect("push accept spawns");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_frames, PushHub};
    use lifescape_json::Value;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Poll a condition for up to 5 s (blocked readers wake on bytes, the
    /// test only waits for the thread to hand them over).
    fn wait_for(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    type Seen = Arc<Mutex<Vec<(String, u64)>>>;

    fn recorder() -> (Seen, Arc<dyn Fn(Value, u64) + Send + Sync>) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let dispatch: Arc<dyn Fn(Value, u64) + Send + Sync> =
            Arc::new(move |value, bytes| sink.lock().unwrap().push((lifescape_json::stringify(&value), bytes)));
        (seen, dispatch)
    }

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

    /// Lot 1.3: the byte count the hub hands over (the received line's
    /// length) is what `res-stats` counted before from a re-serialisation
    /// of the parsed frame, on every shape of frame a worker writes (full
    /// state, light flow, event, report; escapes, unicode, floats,
    /// negatives, large ints, empty containers).
    #[test]
    fn the_received_line_length_equals_the_reserialised_length() {
        use lifescape_json::{self as json, object, str_value, Value};
        let snapshot = object(vec![
            ("account", str_value("amy_gray")),
            ("position", Value::Arr(vec![Value::Int(0), Value::Int(-3222), Value::Int(3218)])),
            ("zone", str_value("Lumbridge \"swamp\" \u{e9}\u{4e16} \\ \n tab\t")),
            ("updated", Value::Float(1795477612.123456)),
            ("speed", Value::Float(1.0)),
            ("tiny", Value::Float(0.000001234)),
            ("big", Value::Int(i64::MAX)),
            ("neg", Value::Int(i64::MIN + 1)),
            ("empty_arr", Value::Arr(Vec::new())),
            ("empty_obj", Value::Obj(Default::default())),
            ("flag", Value::Bool(true)),
            ("none", Value::Null),
            ("trail", Value::Arr((0..300).map(|n| Value::Arr(vec![Value::Int(3200 + n), Value::Int(3200 - n), Value::Int(0)])).collect())),
            ("inv", Value::Arr(vec![object(vec![("name", str_value("coins")), ("count", Value::Int(5_000_000))])])),
        ]);
        let frames = vec![
            object(vec![("type", str_value("snapshot")), ("account", str_value("amy_gray")), ("snapshot", snapshot.clone())]),
            lifescape_orch_session::heartbeat_flows::light_value(
                "amy_gray", "mining", "running", (0, 3228, 3298), 100, "Lumbridge", "fight", 1795477612.5,
                Value::Arr(vec![Value::Arr(vec![Value::Int(837), Value::Int(3231), Value::Int(3301)])]),
                Value::Arr(vec![Value::Arr(vec![str_value("will_field"), Value::Int(3229), Value::Int(3301)])]),
            ),
            object(vec![("type", str_value("event")), ("account", str_value("amy_gray")), ("event", str_value("maze")), ("resolved", Value::Bool(false)), ("tick", Value::Int(77)), ("message", str_value("na\u{ef}ve \"quoted\""))]),
            object(vec![("type", str_value("task_report")), ("account", str_value("amy_gray")), ("report", object(vec![("stop", str_value("goal_reached")), ("ticks", Value::Int(1501)), ("flows", Value::Arr(Vec::new()))]))]),
        ];
        let hub = Arc::new(PushHub::new());
        let (mut worker_end, daemon_end) = UnixStream::pair().expect("socket pair");
        let seen: Arc<Mutex<Vec<(Value, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let dispatch: Arc<dyn Fn(Value, u64) + Send + Sync> = Arc::new(move |value, bytes| sink.lock().unwrap().push((value, bytes)));
        hub.attach("amy_gray".to_string(), daemon_end, dispatch).expect("reader starts");
        let mut lines = Vec::new();
        for frame in &frames {
            let line = json::stringify(frame);
            worker_end.write_all(line.as_bytes()).expect("write");
            worker_end.write_all(b"\n").expect("write");
            lines.push(line);
        }
        wait_for(|| seen.lock().unwrap().len() == frames.len());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), frames.len(), "every frame reaches the dispatch");
        for ((value, bytes), line) in seen.iter().zip(&lines) {
            assert_eq!(*bytes, line.len() as u64);
            assert_eq!(*bytes, json::stringify(value).len() as u64, "same count as the re-serialisation: {}", &line[..line.len().min(80)]);
        }
    }

    /// A runaway line resets instead of growing the hub forever.
    #[test]
    fn runaway_line_resets_loudly() {
        let mut buffer = vec![b'x'; 1_048_577];
        let frames = extract_frames(&mut buffer);
        assert!(frames.is_empty(), "runaway dropped, not dispatched");
        assert!(buffer.is_empty());
    }

    /// Lot 4.1: frames split across writes and several per write reach the
    /// dispatch whole and in order, one account per blocked reader.
    #[test]
    fn partial_and_batched_writes_arrive_whole_and_in_order() {
        let hub = Arc::new(PushHub::new());
        let (seen, dispatch) = recorder();
        let (mut worker_end, daemon_end) = UnixStream::pair().expect("pair");
        hub.attach("t01".to_string(), daemon_end, dispatch).expect("attach");
        worker_end.write_all(b"{\"n\":1}\n{\"n\":2}\n{\"n\"").unwrap();
        wait_for(|| seen.lock().unwrap().len() == 2);
        worker_end.write_all(b":3}\n").unwrap();
        wait_for(|| seen.lock().unwrap().len() == 3);
        let seen = seen.lock().unwrap();
        let order: Vec<&str> = seen.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(order, vec!["{\"n\": 1}", "{\"n\": 2}", "{\"n\": 3}"]);
        assert_eq!(seen[0].1, 7);
    }

    /// An EOF reaps the account; a reconnection replaces the previous
    /// stream (its reader ends) and the old reader never reaps its successor.
    #[test]
    fn eof_reaps_and_a_reconnection_replaces() {
        let hub = Arc::new(PushHub::new());
        let (seen, dispatch) = recorder();
        let (old_worker, old_daemon) = UnixStream::pair().expect("pair");
        hub.attach("t01".to_string(), old_daemon, dispatch.clone()).expect("attach");
        let (mut new_worker, new_daemon) = UnixStream::pair().expect("pair");
        hub.attach("t01".to_string(), new_daemon, dispatch.clone()).expect("attach again");
        // The old reader ends at the shutdown and must leave the new entry alone.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(hub.connected(), vec!["t01".to_string()]);
        drop(old_worker);
        new_worker.write_all(b"{\"live\":true}\n").unwrap();
        wait_for(|| seen.lock().unwrap().len() == 1);
        assert_eq!(seen.lock().unwrap().len(), 1, "the new stream still delivers");
        drop(new_worker);
        wait_for(|| hub.connected().is_empty());
        assert!(hub.connected().is_empty(), "EOF reaps the account");
    }

    /// 74 workers writing at once: every frame of every account arrives,
    /// per-account order kept.
    #[test]
    fn many_accounts_deliver_concurrently_in_per_account_order() {
        let hub = Arc::new(PushHub::new());
        let (seen, dispatch) = recorder();
        let mut workers = Vec::new();
        for index in 0..74 {
            let (worker_end, daemon_end) = UnixStream::pair().expect("pair");
            hub.attach(format!("b{index:02}"), daemon_end, dispatch.clone()).expect("attach");
            workers.push((index, worker_end));
        }
        let writers: Vec<_> = workers
            .into_iter()
            .map(|(index, mut stream)| {
                std::thread::spawn(move || {
                    for n in 0..20 {
                        stream.write_all(format!("{{\"a\":{index},\"n\":{n}}}\n").as_bytes()).unwrap();
                    }
                    stream
                })
            })
            .collect();
        let _keep: Vec<_> = writers.into_iter().map(|handle| handle.join().unwrap()).collect();
        wait_for(|| seen.lock().unwrap().len() == 74 * 20);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 74 * 20);
        for index in 0..74 {
            let own: Vec<String> = seen.iter().filter(|(text, _)| text.starts_with(&format!("{{\"a\": {index},"))).map(|(text, _)| text.clone()).collect();
            let expected: Vec<String> = (0..20).map(|n| format!("{{\"a\": {index}, \"n\": {n}}}")).collect();
            assert_eq!(own, expected, "account b{index:02}");
        }
    }
}
