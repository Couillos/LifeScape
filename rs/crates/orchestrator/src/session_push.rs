#![forbid(unsafe_code)]
//! Worker-to-daemon push frames over the existing daemon unix socket.
//!
//! The session socket stays 1 req/1 reply JSON-lines; the worker also
//! dials the daemon socket (`run/orchestrator.sock`) to PUSH its live
//! state and its events without touching the disk. Every send is
//! fire-and-forget with a short bound: a lost frame is no worse than a
//! missed heartbeat, the next pump pushes the full state again.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};
use crate::paths;

/// One push never holds the pump longer than this.
const PUSH_TIMEOUT: Duration = Duration::from_secs(2);

fn send_push(frame: &Value) {
    let Ok(stream) = UnixStream::connect(paths::socket_path()) else {
        return;
    };
    if stream.set_read_timeout(Some(PUSH_TIMEOUT)).is_err() {
        return;
    }
    if stream.set_write_timeout(Some(PUSH_TIMEOUT)).is_err() {
        return;
    }
    let mut buf = json::stringify(frame);
    buf.push('\n');
    if (&stream).write_all(buf.as_bytes()).is_err() {
        return;
    }
    // The reply is protocol hygiene only; nobody waits on it.
    let mut line = String::new();
    let _ = BufReader::new(&stream).read_line(&mut line);
}

/// Full live state frame. The caller throttles (~10 ticks): the daemon
/// serves the front from memory, the disk stays the last-breath fallback.
pub(crate) fn snapshot_frame(account: &str, snapshot: &Value) -> Value {
    object(vec![
        ("type", str_value("snapshot")),
        ("account", str_value(account)),
        ("snapshot", snapshot.clone()),
    ])
}

/// Push the full live state to the daemon (fire-and-forget).
pub(crate) fn push_snapshot(account: &str, snapshot: &Value) {
    send_push(&snapshot_frame(account, snapshot));
}

/// Push a finished task report to the daemon (fire-and-forget, like any
/// other push frame: a lost report is a missed heartbeat line, the next
/// run pushes again — and the session log keeps the same JSON).
pub(crate) fn push_task_report(account: &str, report: &Value) {
    send_push(&object(vec![
        ("type", str_value("task_report")),
        ("account", str_value(account)),
        ("report", report.clone()),
    ]));
}

/// Structured event frame: tick + position ride along, the log keeps only
/// the human line. `args` is the running script's args (empty for inline
/// rescues with none). `message` carries the error text for `script_error`,
/// empty for random events.
pub(crate) fn event_frame(
    account: &str,
    kind: &str,
    event: &str,
    resolved: bool,
    tick: u64,
    script: &str,
    args: &[String],
    message: &str,
    pos: (i32, i32, i32),
) -> Value {
    object(vec![
        ("type", str_value("event")),
        ("account", str_value(account)),
        ("kind", str_value(kind)),
        ("event", str_value(event)),
        ("resolved", Value::Bool(resolved)),
        ("tick", Value::Int(tick as i64)),
        ("script", str_value(script)),
        (
            "args",
            Value::Arr(args.iter().map(|arg| str_value(arg)).collect()),
        ),
        ("message", str_value(message)),
        (
            "at",
            Value::Arr(vec![
                Value::Int(i64::from(pos.0)),
                Value::Int(i64::from(pos.1)),
                Value::Int(i64::from(pos.2)),
            ]),
        ),
    ])
}

/// Push a structured event to the daemon (fire-and-forget).
pub(crate) fn push_event(
    account: &str,
    kind: &str,
    event: &str,
    resolved: bool,
    tick: u64,
    script: &str,
    args: &[String],
    message: &str,
    pos: (i32, i32, i32),
) {
    send_push(&event_frame(account, kind, event, resolved, tick, script, args, message, pos));
}

#[cfg(test)]
mod tests {
    use super::{event_frame, snapshot_frame};
    use lifescape_json::{Value, object, str_value};

    #[test]
    fn event_frame_carries_tick_and_position() {
        let frame = event_frame("t01", "random_event", "maze", false, 123, "rescue_maze", &[], "", (0, 2900, 4560));
        assert_eq!(frame.get("type").as_str(), Some("event"));
        assert_eq!(frame.get("account").as_str(), Some("t01"));
        assert_eq!(frame.get("tick").as_i64(), Some(123));
        assert_eq!(frame.get("event").as_str(), Some("maze"));
        assert_eq!(frame.get("resolved").as_bool(), Some(false));
        assert!(frame.get("args").as_arr().cloned().unwrap_or_default().is_empty());
        let at = frame.get("at").as_arr().cloned().unwrap_or_default();
        assert_eq!(at.len(), 3);
        assert_eq!(at[1].as_i64(), Some(2900));
    }

    #[test]
    fn event_frame_carries_script_args() {
        let args = vec!["3200".to_string(), "3200".to_string()];
        let frame = event_frame("t01", "random_event", "maze", false, 9, "fishing", &args, "", (0, 3200, 3200));
        let carried = frame.get("args").as_arr().cloned().unwrap_or_default();
        assert_eq!(carried.len(), 2);
        assert_eq!(carried[0].as_str(), Some("3200"));
        assert_eq!(frame.get("script").as_str(), Some("fishing"));
    }

    #[test]
    fn snapshot_frame_wraps_state_untouched() {
        let state = object(vec![("ticks", Value::Int(7))]);
        let frame = snapshot_frame("t01", &state);
        assert_eq!(frame.get("type").as_str(), Some("snapshot"));
        assert_eq!(frame.get("snapshot").get("ticks").as_i64(), Some(7));
        assert_eq!(str_value("x").as_str(), Some("x"));
    }
}
