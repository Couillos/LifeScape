#![forbid(unsafe_code)]
//! Worker-side persistent push (E2): one `push.sock` stream per worker
//! process, held for life, reconnected when the daemon restarts.
//!
//! Frames stay fire-and-forget (no reply read): the daemon already treats
//! a lost frame like a missed heartbeat. When the hub is unreachable
//! (old daemon), each send falls back to one legacy one-shot frame on
//! the CLI socket so rolling upgrades never lose telemetry.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use lifescape_json::{self as json, Value};

/// Short write bound: a push never holds the pump longer than this.
const PUSH_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

struct HeldConn {
    stream: Option<UnixStream>,
}

fn held() -> &'static Mutex<HeldConn> {
    static HELD: OnceLock<Mutex<HeldConn>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(HeldConn { stream: None }))
}

/// The commands the daemon sent on the held stream, waiting for the serve
/// loop (lot 5.9). The loop serves them exactly as it serves a per-call
/// connection (it accepts, then a thread runs the command): a worker whose
/// loop is wedged answers neither, so the daemon's call timeouts still
/// tell a wedged worker from a healthy one.
struct Inbox {
    commands: Mutex<VecDeque<(u64, Value)>>,
    arrived: Condvar,
}

fn inbox() -> &'static Inbox {
    static INBOX: OnceLock<Inbox> = OnceLock::new();
    INBOX.get_or_init(|| Inbox { commands: Mutex::new(VecDeque::new()), arrived: Condvar::new() })
}

/// The worker serves commands from the held stream: from now on its hello
/// says `durable` and a reader thread queues the daemon's commands for the
/// serve loop. Called by the session before its first push (a stream
/// dialled before it stays a push-only one, the daemon keeps its per-call
/// path).
pub fn enable_commands() {
    DURABLE.store(true, std::sync::atomic::Ordering::SeqCst);
}

static DURABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn durable() -> bool {
    DURABLE.load(std::sync::atomic::Ordering::SeqCst)
}

/// The commands that arrived since the last call (oldest first).
pub fn take_commands() -> Vec<(u64, Value)> {
    inbox().commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..).collect()
}

/// The serve loop's rest: up to `rest`, cut short when a command arrives
/// (the per-call path waited the whole rest, 100 ms to 1 s, for its accept).
pub fn rest(rest: Duration) {
    let inbox = inbox();
    let queued = inbox.commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if queued.is_empty() {
        let _ = inbox.arrived.wait_timeout(queued, rest);
    }
}

fn hello_frame(account: &str) -> String {
    let mut frame = json::stringify(&lifescape_orch_fleet::frame_codec::encode_hello(account, durable()));
    frame.push('\n');
    frame
}

fn dial_hub(account: &str) -> Option<UnixStream> {
    let stream = UnixStream::connect(crate::paths::push_socket_path()).ok()?;
    stream.set_write_timeout(Some(PUSH_WRITE_TIMEOUT)).ok()?;
    (&stream).write_all(hello_frame(account).as_bytes()).ok()?;
    if durable() {
        // The daemon's commands arrive on this same stream (lot 5.9).
        if let Ok(reader) = stream.try_clone() {
            let spawned = std::thread::Builder::new().name("session-commands".to_string()).spawn(move || command_reader(reader));
            if spawned.is_err() {
                crate::logln!("push: command reader not started, the daemon keeps its per-call path");
            }
        }
    }
    Some(stream)
}

/// Blocked in `read` on the held stream: one command frame per line
/// (`frame_codec::decode_command`), queued for the serve loop. Ends with
/// the stream.
fn command_reader(stream: UnixStream) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let Ok(frame) = json::parse(&line) else { continue };
        let Some(command) = lifescape_orch_fleet::frame_codec::decode_command(&frame) else { continue };
        let inbox = inbox();
        inbox.commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push_back(command);
        inbox.arrived.notify_all();
    }
}

/// Writes one reply frame on the held stream (the daemon matches it to its
/// waiting call by id; a reply for a connection that is gone is dropped
/// there). Shares the held-stream lock with the push frames.
pub fn send_reply(id: u64, reply: &Value) {
    let mut wire = json::stringify(&lifescape_orch_fleet::frame_codec::encode_reply(id, reply));
    wire.push('\n');
    let guard = held().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(stream) = guard.stream.as_ref() {
        let _ = (&*stream).write_all(wire.as_bytes());
    }
}

fn legacy_one_shot(frame: &str) {
    let Ok(stream) = UnixStream::connect(crate::paths::socket_path()) else {
        return;
    };
    if stream.set_read_timeout(Some(PUSH_WRITE_TIMEOUT)).is_err() {
        return;
    }
    if stream.set_write_timeout(Some(PUSH_WRITE_TIMEOUT)).is_err() {
        return;
    }
    // Fire-and-forget: the reply is never awaited (reading it held the
    // sender up to 2 s per frame while the daemon was busy restoring).
    let _ = (&stream).write_all(frame.as_bytes());
}

/// Snapshots waiting for the sender beyond this are dropped oldest first
/// (the next one supersedes them anyway).
const BULK_CAP: usize = 64;

/// Frames waiting for the sender thread: vital ones (task reports,
/// events) are never dropped, bulk ones (snapshots) keep the newest.
struct Outbox {
    vital: VecDeque<(String, String)>,
    bulk: VecDeque<(String, String)>,
}

fn outbox() -> &'static (Mutex<Outbox>, Condvar) {
    static OUTBOX: OnceLock<(Mutex<Outbox>, Condvar)> = OnceLock::new();
    OUTBOX.get_or_init(|| {
        std::thread::Builder::new()
            .name("push-sender".to_string())
            .spawn(sender_loop)
            .expect("push sender spawns");
        (Mutex::new(Outbox { vital: VecDeque::new(), bulk: VecDeque::new() }), Condvar::new())
    })
}

fn sender_loop() {
    loop {
        let next = {
            let (lock, ready) = outbox();
            let mut queued = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if let Some(frame) = queued.vital.pop_front().or_else(|| queued.bulk.pop_front()) {
                    break frame;
                }
                queued = ready.wait(queued).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        deliver(&next.0, &next.1);
    }
}

/// Queues one frame for the push sender and returns at once: the pump
/// (which holds the bot) never waits on the daemon. Live 2026-11-13:
/// during each daemon restart, synchronous pushes froze every worker's
/// pump 2 s per frame — server disconnects, 5 s supervisor probes timing
/// out, ~3–4 min re-adoption. Task reports and events are never dropped.
pub fn send_frame(account: &str, frame: &Value) {
    let mut wire = json::stringify(frame);
    wire.push('\n');
    let vital = matches!(frame.get("type").as_str(), Some("task_report") | Some("event"));
    let (lock, ready) = outbox();
    let mut queued = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if vital {
        queued.vital.push_back((account.to_string(), wire));
    } else {
        if queued.bulk.len() >= BULK_CAP {
            queued.bulk.pop_front();
        }
        queued.bulk.push_back((account.to_string(), wire));
    }
    ready.notify_one();
}

/// Send one wire frame on the held hub stream, dialling (or redialling
/// after a daemon restart) as needed. Falls back to a legacy one-shot
/// when no hub answers. Runs on the sender thread only.
fn deliver(account: &str, wire: &str) {
    let mut guard = held().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.stream.is_none() {
        guard.stream = dial_hub(account);
    }
    if let Some(stream) = guard.stream.as_ref() {
        if (&*stream).write_all(wire.as_bytes()).is_ok() {
            crate::res_stats::record_push(wire.len() as u64);
            return;
        }
        // Held stream died (daemon restart): drop it, count the
        // reconnect, redial once below.
        guard.stream = None;
        drop(guard);
        crate::res_stats::record_push_reconnect();
    } else {
        drop(guard);
    }
    // Hub down (or write failed): one reconnect attempt, then legacy.
    if let Some(stream) = dial_hub(account) {
        if (&stream).write_all(wire.as_bytes()).is_ok() {
            held().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).stream = Some(stream);
            crate::res_stats::record_push(wire.len() as u64);
            return;
        }
    }
    legacy_one_shot(&wire);
}

#[cfg(test)]
mod tests {
    use super::send_frame;
    use lifescape_json::{object, str_value};

    /// With no hub and no daemon listening, a push degrades to silence
    /// (fire-and-forget): it must never panic the pump thread.
    #[test]
    fn push_without_daemon_is_silent() {
        let frame = object(vec![("type", str_value("snapshot"))]);
        send_frame("pushconn-absent-test", &frame);
    }

    /// Queuing never waits on the daemon: hundreds of frames return at
    /// once whatever the sender is doing (it held the pump before).
    #[test]
    fn queuing_never_blocks_the_pump() {
        let started = std::time::Instant::now();
        for _ in 0..500 {
            send_frame("pushconn-burst-test", &object(vec![("type", str_value("snapshot_light"))]));
        }
        send_frame("pushconn-burst-test", &object(vec![("type", str_value("task_report"))]));
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "took {:?}", started.elapsed());
        let (lock, _) = super::outbox();
        let queued = lock.lock().unwrap();
        assert!(queued.bulk.len() <= super::BULK_CAP, "bulk bounded: {}", queued.bulk.len());
    }

    /// Hello names the account so the hub files the stream with no
    /// extra round-trip.
    #[test]
    fn hello_names_the_account() {
        let hello = super::hello_frame("t07");
        assert!(hello.ends_with('\n'));
        assert!(hello.contains("\"t07\""), "hub files by account, got {hello}");
        assert!(hello.contains("\"push\""));
    }

    /// Lot 5.9: the daemon's command frames, read off the stream, wait in
    /// the inbox for the serve loop and cut its rest short; frames of
    /// other kinds are ignored.
    #[test]
    fn commands_read_off_the_stream_wake_the_serve_loop() {
        use std::io::Write;
        let (mut daemon_end, worker_end) = std::os::unix::net::UnixStream::pair().expect("pair");
        std::thread::spawn(move || super::command_reader(worker_end));
        let request = lifescape_json::object(vec![("cmd", str_value("ping"))]);
        let mut wire = lifescape_json::stringify(&lifescape_orch_fleet::frame_codec::encode_command(41, &request));
        wire.push('\n');
        daemon_end.write_all(b"{\"type\":\"snapshot\"}\nnot json\n").unwrap();
        daemon_end.write_all(wire.as_bytes()).unwrap();
        let started = std::time::Instant::now();
        super::rest(std::time::Duration::from_secs(10));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "the rest was cut short by the command");
        assert_eq!(super::take_commands(), vec![(41, request)]);
        assert!(super::take_commands().is_empty());
    }
}
