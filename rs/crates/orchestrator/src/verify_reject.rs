//! W4 fast fail: explicit server rejections end a scenario within 20 ticks.
//!
//! A stuck scenario used to burn its whole tick budget on a rejection the
//! server already named (`I can't reach that!`, a full pack,
//! `UNSET_MAP_FLAG`, a planner `NoRoute`). `RejectWatch` arms on the first
//! rejection seen after the run baseline and fails the run once 20 game
//! ticks pass on the same tile: the verdict carries the rejection as its
//! cause instead of a bare budget dossier. Moving resets the watch, so a
//! retried bank side or a re-walked field (the normal recoveries) never
//! trips it; only a stuck bot does.

#![forbid(unsafe_code)]

use lifescape_json::{self as json, Value};

use crate::verify_assert::position_of;

/// Game ticks a rejection may sit on an unmoved bot before the run fails.
pub const REJECT_GRACE_TICKS: i64 = 20;

/// Newest `message_ticks` in a snapshot (-1 when the window is empty): the
/// baseline watermark, so only strictly newer lines can reject the run.
pub fn max_msg_tick(state: &Value) -> i64 {
    state
        .get("message_ticks")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|tick| tick.as_i64())
        .max()
        .unwrap_or(-1)
}

/// True when one fresh game line names an explicit server rejection.
fn line_rejects(line: &str) -> bool {
    let text = line.to_ascii_lowercase();
    text.contains("can't reach that")
        || text.contains("inventory space")
        || text.contains("too full to hold")
        || text.contains("can't carry any more")
        || text.contains("unset_map_flag")
        || text.contains("unset map flag")
}

fn short(text: String) -> String {
    const LIMIT: usize = 160;
    if text.len() <= LIMIT { text } else { format!("{}...", &text[..LIMIT]) }
}

/// Explicit rejection cause visible in `state` after `since_tick`, if any:
/// a fresh game line, or a fresh `last_error` naming a dead plan. `None`
/// when the window holds only progress or chatter.
pub fn rejection_of(state: &Value, since_tick: i64, baseline_error: &Value) -> Option<String> {
    let messages = state.get("messages").as_arr().cloned().unwrap_or_default();
    let ticks = state.get("message_ticks").as_arr().cloned().unwrap_or_default();
    for (line, tick) in messages.iter().zip(ticks.iter()) {
        let fresh = tick.as_i64().is_some_and(|tick| tick > since_tick);
        if fresh && line.as_str().is_some_and(line_rejects) {
            return Some(short(format!("rejected: {}", line.as_str().unwrap_or("?"))));
        }
    }
    let error = state.get("last_error");
    if !error.is_null() && json::stringify(error) != json::stringify(baseline_error) {
        let lowered = json::stringify(error).to_ascii_lowercase();
        if lowered.contains("noroute") || lowered.contains("no route") || lowered.contains("travel failure") {
            return Some(short(format!("rejected: {}", json::stringify(error))));
        }
    }
    None
}

/// Armed first-rejection state: cause, game tick and tile at arming time.
/// Same tile plus 20 ticks with no terminal verdict means stuck: fail.
pub struct RejectWatch {
    armed: Option<(String, Option<i64>, Option<(i64, i64, i64)>)>,
}

impl RejectWatch {
    pub fn new() -> Self {
        RejectWatch { armed: None }
    }

    /// Feed one `show` snapshot. `Some(cause)` the first poll at least 20
    /// game ticks after the arming sighting with no move since; `None`
    /// otherwise (no rejection yet, bot moved, or grace still running).
    pub fn note(&mut self, state: &Value, since_tick: i64, baseline_error: &Value) -> Option<String> {
        if let Some((cause, first_tick, first_pos)) = self.armed.clone() {
            if position_of(state) != first_pos {
                self.armed = None;
                return None;
            }
            match (state.get("ticks").as_i64(), first_tick) {
                (Some(now), Some(first)) if now.saturating_sub(first) >= REJECT_GRACE_TICKS => return Some(cause),
                // No game clock (pre-stream snapshot): fail on sight
                // rather than burn a budget blind.
                (None, _) => return Some(cause),
                _ => return None,
            }
        }
        if let Some(cause) = rejection_of(state, since_tick, baseline_error) {
            self.armed = Some((cause, state.get("ticks").as_i64(), position_of(state)));
        }
        None
    }
}

impl Default for RejectWatch {
    fn default() -> Self {
        RejectWatch::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json as json;

    fn show(messages: &[(&str, i64)], ticks: i64, pos: (i64, i64, i64)) -> Value {
        let lines: Vec<Value> = messages.iter().map(|(line, _)| json::str_value(line)).collect();
        let stamps: Vec<Value> = messages.iter().map(|(_, tick)| Value::Int(*tick)).collect();
        json::parse(&format!(
            "{{\"account\": \"t40\", \"position\": [{},{},{}], \"ticks\": {}, \"messages\": {}, \"message_ticks\": {}}}",
            pos.0,
            pos.1,
            pos.2,
            ticks,
            json::stringify(&Value::Arr(lines)),
            json::stringify(&Value::Arr(stamps)),
        ))
        .expect("fixture parses")
    }

    /// Every explicit rejection the PRD names is caught with its cause.
    #[test]
    fn fresh_rejections_name_the_cause() {
        let baseline = Value::Null;
        for line in [
            "I can't reach that!",
            "You don't have enough inventory space to do that.",
            "Your inventory is too full to hold any more logs.",
            "You can't carry any more fish.",
        ] {
            let state = show(&[(line, 118)], 120, (0, 3220, 3217));
            let cause = rejection_of(&state, 100, &baseline).expect("rejects");
            assert!(cause.contains("rejected"), "cause loud, got: {cause}");
        }
        for line in ["You get some grain.", "You cook the shrimp.", "Welcome to RuneScape."] {
            let state = show(&[(line, 118)], 120, (0, 3220, 3217));
            assert!(rejection_of(&state, 100, &baseline).is_none(), "progress is not a rejection: {line}");
        }
    }

    /// A rejection at or before the baseline watermark belongs to an older
    /// run: never reject on it.
    #[test]
    fn stale_baseline_lines_never_reject() {
        let state = show(&[("I can't reach that!", 100)], 120, (0, 3220, 3217));
        assert!(rejection_of(&state, 100, &Value::Null).is_none(), "tick == watermark is stale");
    }

    /// A bot stuck on its tile after a rejection fails exactly at the
    /// grace: the Port Sarim door case (loc 1530) ends here, not at budget.
    #[test]
    fn stuck_bot_fails_within_20_ticks() {
        let baseline = Value::Null;
        let mut watch = RejectWatch::new();
        let pos = (0, 3039, 3236);
        assert!(watch.note(&show(&[("I can't reach that!", 101)], 100, pos), 90, &baseline).is_none(), "arming poll never fails");
        for tick in 101..120 {
            assert!(
                watch.note(&show(&[("I can't reach that!", 101)], tick, pos), 90, &baseline).is_none(),
                "grace still running at tick {tick}"
            );
        }
        let cause = watch.note(&show(&[("I can't reach that!", 101)], 120, pos), 90, &baseline).expect("fails at +20");
        assert!(cause.to_ascii_lowercase().contains("can't reach"), "dossier cause names the door: {cause}");
    }

    /// A bot that moves after the rejection is retrying (bank side, next
    /// field): the watch resets instead of failing a recovery.
    #[test]
    fn move_resets_the_grace() {
        let baseline = Value::Null;
        let mut watch = RejectWatch::new();
        assert!(watch.note(&show(&[("I can't reach that!", 101)], 100, (0, 3220, 3217)), 90, &baseline).is_none());
        let clean = show(&[("You open the bank.", 112)], 112, (0, 3221, 3217));
        assert!(watch.note(&clean, 90, &baseline).is_none(), "moved bot resets");
        let parked = show(&[("You open the bank.", 112)], 112, (0, 3221, 3217));
        assert!(watch.note(&parked, 90, &baseline).is_none(), "no rejection armed, never fails");
    }

    /// A fresh planner `NoRoute` in `last_error` rejects even with no new
    /// game line; a stale one (same as baseline) never does.
    #[test]
    fn fresh_noroute_error_rejects() {
        let error = json::parse(r#"{"message": "travel failure NoRoute seg=2"}"#).expect("error parses");
        let mut state = show(&[], 120, (0, 3220, 3217));
        if let Value::Obj(ref mut fields) = state {
            fields.insert("last_error".to_string(), error.clone());
        }
        assert!(rejection_of(&state, 100, &Value::Null).expect("NoRoute rejects").contains("Route"));
        assert!(rejection_of(&state, 100, &error).is_none(), "stale NoRoute is an older run");
    }

    /// No game clock (pre-stream snapshot): fail on sight rather than burn
    /// a budget blind.
    #[test]
    fn missing_clock_fails_on_sight() {
        let mut state = show(&[("I can't reach that!", 5)], 7, (0, 3220, 3217));
        if let Value::Obj(ref mut fields) = state {
            fields.remove("ticks");
        }
        let mut watch = RejectWatch::new();
        assert!(watch.note(&state, 0, &Value::Null).is_none(), "first sighting arms");
        assert!(watch.note(&state, 0, &Value::Null).is_some(), "no clock, no grace to count");
    }

    /// The watermark reads the newest stamp, -1 on an empty window.
    #[test]
    fn watermark_reads_newest_stamp() {
        assert_eq!(max_msg_tick(&show(&[("a", 3), ("b", 9), ("c", 4)], 9, (0, 0, 0))), 9);
        assert_eq!(max_msg_tick(&show(&[], 9, (0, 0, 0))), -1);
    }
}
