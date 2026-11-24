#![forbid(unsafe_code)]
//! Worker-to-daemon push frames over the existing daemon unix socket.
//!
//! The session socket stays 1 req/1 reply JSON-lines; the worker also
//! dials the daemon socket (`run/orchestrator.sock`) to PUSH its live
//! state and its events without touching the disk. Every send is
//! fire-and-forget with a short bound: a lost frame is no worse than a
//! missed heartbeat, the next pump pushes the full state again.

use lifescape_json::{Value, str_value};

use crate::frame_codec::{EventFrame, ReportFrame, SnapshotFrame};
use crate::job_state::{Phase, Status};

fn send_push(frame: &Value) {
    // E2 persistent hub first (one stream per worker life); legacy
    // one-shot only when no hub answers (rolling upgrades).
    let account = frame.get("account").as_str().unwrap_or("").to_string();
    crate::push_conn::send_frame(&account, frame);
}

/// Full live state frame. The caller throttles (~10 ticks): the daemon
/// serves the front from memory, the disk stays the last-breath fallback.
///
/// Lot 4.2: the envelope (outside the snapshot tree, which stays
/// untouched) carries the session's `phase` and `status` and the send
/// time, so the daemon reads them from the frame instead of a `status`
/// round trip.
pub(crate) fn snapshot_frame(account: &str, snapshot: &Value, phase: &str, status: &str) -> Value {
    SnapshotFrame {
        account: account.to_string(),
        phase: Some(Phase::parse(phase)),
        status: Some(Status::parse(status)),
        sent: Some(crate::session::now_secs()),
        snapshot: snapshot.clone(),
    }
    .into_value()
}

/// Push the full live state to the daemon (fire-and-forget).
pub(crate) fn push_snapshot(account: &str, snapshot: &Value, phase: &str, status: &str) {
    send_push(&snapshot_frame(account, snapshot, phase, status));
}

/// A light frame with the session `phase` added (lot 4.2; `status`,
/// `script` and `updated` already ride the light frame).
pub(crate) fn light_with_phase(mut light: Value, phase: &str) -> Value {
    if let Value::Obj(map) = &mut light {
        map.insert("phase".to_string(), str_value(phase));
    }
    light
}

/// Push a light heartbeat to the daemon (E4 two-flow: position and
/// status only, merged into the last full state daemon-side).
pub(crate) fn push_light(light: Value, phase: &str) {
    send_push(&light_with_phase(light, phase));
}

/// Push a finished task report to the daemon (fire-and-forget, like any
/// other push frame: a lost report is a missed heartbeat line, the next
/// run pushes again — and the session log keeps the same JSON).
pub(crate) fn push_task_report(account: &str, report: &Value) {
    send_push(&ReportFrame { account: account.to_string(), report: report.clone() }.encode());
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
    EventFrame {
        account: account.to_string(),
        kind: kind.to_string(),
        event: event.to_string(),
        resolved,
        tick: tick as i64,
        script: script.to_string(),
        args: args.iter().map(|arg| str_value(arg)).collect(),
        message: message.to_string(),
        at: Value::Arr(vec![Value::Int(i64::from(pos.0)), Value::Int(i64::from(pos.1)), Value::Int(i64::from(pos.2))]),
    }
    .encode()
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
    use super::{event_frame, light_with_phase, snapshot_frame};
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
        let frame = snapshot_frame("t01", &state, "idle", "idle");
        assert_eq!(frame.get("type").as_str(), Some("snapshot"));
        assert_eq!(frame.get("snapshot").get("ticks").as_i64(), Some(7));
        assert_eq!(str_value("x").as_str(), Some("x"));
        assert_eq!(frame.get("phase").as_str(), Some("idle"));
        assert_eq!(frame.get("status").as_str(), Some("idle"));
        assert!(frame.get("sent").as_f64().is_some_and(|at| at > 1.0e9));
    }

    /// Lot 4.2: the light frame keeps every key it had and gains `phase`.
    #[test]
    fn light_frame_gains_the_phase_only() {
        let light = lifescape_json::object(vec![("type", str_value("snapshot_light")), ("status", str_value("running"))]);
        let framed = light_with_phase(light.clone(), "running");
        assert_eq!(framed.get("phase").as_str(), Some("running"));
        assert_eq!(framed.get("status").as_str(), Some("running"));
        assert_eq!(framed.get("type").as_str(), Some("snapshot_light"));
    }

    /// Lot 5.2: what the worker's encoders write is what the daemon's
    /// decoder reads back, key for key (every frame type).
    #[test]
    fn the_workers_frames_round_trip_through_the_codec() {
        use crate::frame_codec::Frame;
        let state = object(vec![("account", str_value("t01")), ("ticks", Value::Int(5)), ("position", Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3218)]))]);
        let light = crate::heartbeat_flows::light_value("t01", "mining", "running", (0, 3228, 3298), 100, "Lumbridge", "fight", 1795477612.5, Value::Arr(vec![Value::Arr(vec![Value::Int(837), Value::Int(3231), Value::Int(3301)])]), Value::Arr(Vec::new()));
        let frames = vec![
            snapshot_frame("t01", &state, "running", "running"),
            light_with_phase(light.clone(), "running"),
            light,
            event_frame("t01", "script_error", "unknown", true, 9, "fishing", &["a".to_string()], "boom", (0, 3200, 3200)),
            object(vec![("type", str_value("task_report")), ("account", str_value("t01")), ("report", object(vec![("stop", str_value("goal_reached"))]))]),
        ];
        for frame in frames {
            let decoded = Frame::decode(frame.clone()).expect("a frame the daemon takes");
            assert_eq!(decoded.encode(), frame);
        }
    }
}
