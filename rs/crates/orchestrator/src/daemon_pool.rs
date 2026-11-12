#![forbid(unsafe_code)]
//! Bounded daemon socket pool (E2): 8 named workers drain one queue of
//! 64, instead of one thread per accepted connection.
//!
//! Before: `main.rs` spawned a `daemon-conn` thread per `accept` — at 300
//! bots the daemon churned hundreds of short-lived threads per second
//! (pushes + 35 nav polls per plan each rode their own connection).
//! The pool bounds threads AND queue depth: past 64 waiting streams the
//! listener drops the connection loudly (the CLI retries, workers use
//! the persistent hub), never spawns.

use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use lifescape_json::Value;

/// Pool threads serving daemon socket streams.
pub const POOL_THREADS: usize = 8;
/// Waiting streams past this are refused loudly, never queued unbounded.
pub const POOL_QUEUE: usize = 64;
/// Slow-client guard per stream, same as before.
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// Serve `listener` on the pool: the accept thread only enqueues.
/// `serve_one` answers exactly one request then closes (CLI shape);
/// persistent worker pushes ride `push.sock` (`push_hub`), not this.
pub fn serve_pooled(
    listener: UnixListener,
    serve_one: Arc<dyn Fn(UnixStream) + Send + Sync>,
    keep_running: Arc<dyn Fn() -> bool + Send + Sync>,
) {
    let (sender, receiver) = mpsc::sync_channel::<UnixStream>(POOL_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..POOL_THREADS {
        let worker_receiver = receiver.clone();
        let worker_serve = serve_one.clone();
        std::thread::Builder::new()
            .name(format!("daemon-pool-{index}"))
            .spawn(move || loop {
                let stream = {
                    let guard = worker_receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.recv().map_err(|_| ()).map(|stream| stream)
                };
                match stream {
                    Ok(stream) => worker_serve(stream),
                    Err(()) => return,
                }
            })
            .expect("daemon pool spawns");
    }
    let accepted = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let refused = Arc::new(std::sync::atomic::AtomicU64::new(0));
    for stream in listener.incoming() {
        // `shutdown` clears the flag then connects once to wake this
        // accept: the loop ends and the daemon exits (it never did).
        if !keep_running() {
            break;
        }
        let Ok(stream) = stream else { break };
        let _ = stream.set_read_timeout(Some(STREAM_TIMEOUT));
        match sender.try_send(stream) {
            Ok(()) => {
                accepted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            Err(mpsc::TrySendError::Full(_)) | Err(mpsc::TrySendError::Disconnected(_)) => {
                refused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                eprintln!(
                    "daemon pool full (accepted={} refused={}): dropping connection loudly",
                    accepted.load(std::sync::atomic::Ordering::Relaxed),
                    refused.load(std::sync::atomic::Ordering::Relaxed),
                );
            }
        }
    }
}

/// Read exactly one newline frame with the stream timeout. Shared by the
/// pool workers; `None` means no usable request (EOF or bad JSON shape
/// is answered by the caller, never a silent drop).
pub fn read_frame(stream: &UnixStream) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let mut line = String::new();
    match BufReader::new(stream).read_line(&mut line) {
        Ok(0) => None,
        Ok(_) if line.trim().is_empty() => Some(Value::Obj(Default::default())),
        Ok(_) => lifescape_json::parse(&line).ok(),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::read_frame;
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    /// One frame reads back exactly; EOF reads `None` (caller closes).
    #[test]
    fn frame_round_trip_then_eof() {
        let (mut left, right) = UnixStream::pair().unwrap();
        left.write_all(b"{\"cmd\":\"status\"}\n").unwrap();
        let frame = read_frame(&right).expect("frame reads");
        assert_eq!(frame.get("cmd").as_str(), Some("status"));
        drop(left);
        assert!(read_frame(&right).is_none(), "EOF closes, never spins");
    }
}
