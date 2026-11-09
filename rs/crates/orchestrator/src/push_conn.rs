#![forbid(unsafe_code)]
//! Worker-side persistent push (E2): one `push.sock` stream per worker
//! process, held for life, reconnected when the daemon restarts.
//!
//! Frames stay fire-and-forget (no reply read): the daemon already treats
//! a lost frame like a missed heartbeat. When the hub is unreachable
//! (old daemon), each send falls back to one legacy one-shot frame on
//! the CLI socket so rolling upgrades never lose telemetry.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::{Mutex, OnceLock};
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

fn hello_frame(account: &str) -> String {
    let mut frame = json::stringify(&lifescape_json::object(vec![
        ("hello", lifescape_json::str_value("push")),
        ("account", lifescape_json::str_value(account)),
    ]));
    frame.push('\n');
    frame
}

fn dial_hub(account: &str) -> Option<UnixStream> {
    let stream = UnixStream::connect(crate::push_hub::socket_path()).ok()?;
    stream.set_write_timeout(Some(PUSH_WRITE_TIMEOUT)).ok()?;
    (&stream).write_all(hello_frame(account).as_bytes()).ok()?;
    Some(stream)
}

fn legacy_one_shot(frame: &str) {
    use std::io::{BufRead, BufReader};
    let Ok(stream) = UnixStream::connect(crate::paths::socket_path()) else {
        return;
    };
    if stream.set_read_timeout(Some(PUSH_WRITE_TIMEOUT)).is_err() {
        return;
    }
    if stream.set_write_timeout(Some(PUSH_WRITE_TIMEOUT)).is_err() {
        return;
    }
    if (&stream).write_all(frame.as_bytes()).is_err() {
        return;
    }
    let mut line = String::new();
    let _ = BufReader::new(&stream).read_line(&mut line);
}

/// Send one frame on the held hub stream, dialling (or redialling after
/// a daemon restart) as needed. Falls back to a legacy one-shot when no
/// hub answers.
pub fn send_frame(account: &str, frame: &Value) {
    let mut wire = json::stringify(frame);
    wire.push('\n');
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

    /// Hello names the account so the hub files the stream with no
    /// extra round-trip.
    #[test]
    fn hello_names_the_account() {
        let hello = super::hello_frame("t07");
        assert!(hello.ends_with('\n'));
        assert!(hello.contains("\"t07\""), "hub files by account, got {hello}");
        assert!(hello.contains("\"push\""));
    }
}
