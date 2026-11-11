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
/// `last_ticks` is the newest game tick seen on the previous poll
/// (baseline watermark before the first): only a rejection NEWER than
/// this arms the watch. The daemon serves message views in bursts
/// dozens of ticks late, so any absolute floor (baseline or consumed
/// watermark) arms on already-recovered refusals — the cook stand
/// probe, the full pack — and the grace then kills the next normal
/// park (cooking, bank heal). Newness is relative to the previous
/// poll, never absolute: a recovered line is old on arrival, a stuck
/// bot re-emits fresh ones on every retry. The armed grace below is
/// unchanged, so a truly stuck bot still fails within 20 ticks.
pub struct RejectWatch {
    armed: Option<(String, Option<i64>, Option<(i64, i64, i64)>)>,
    last_ticks: Option<i64>,
}

impl RejectWatch {
    pub fn new() -> Self {
        RejectWatch { armed: None, last_ticks: None }
    }

    /// Feed one `show` snapshot. `Some(cause)` the first poll at least 20
    /// game ticks after the arming sighting with no move since; `None`
    /// otherwise (no rejection yet, bot moved, or grace still running).
    pub fn note(&mut self, state: &Value, since_tick: i64, baseline_error: &Value) -> Option<String> {
        let now_ticks = state.get("ticks").as_i64();
        if let Some((cause, first_tick, first_pos)) = self.armed.clone() {
            if position_of(state) != first_pos {
                self.armed = None;
            } else {
                let done = match (now_ticks, first_tick) {
                    (Some(now), Some(first)) => now.saturating_sub(first) >= REJECT_GRACE_TICKS,
                    // No game clock (pre-stream snapshot): fail on sight
                    // rather than burn a budget blind.
                    (None, _) => true,
                    _ => false,
                };
                self.last_ticks = now_ticks.or(self.last_ticks);
                if done {
                    return Some(cause);
                }
                return None;
            }
        }
        // Newer than the previous poll (baseline before the first): a
        // lagged burst carries only old lines and never arms, while a
        // stuck bot's retried refusal is fresh on arrival. Capped below
        // by the baseline so a pre-run line never arms the first poll.
        let threshold = self.last_ticks.max(Some(since_tick)).unwrap_or(since_tick);
        if let Some(cause) = rejection_of(state, threshold, baseline_error) {
            self.armed = Some((cause, now_ticks, position_of(state)));
        }
        self.last_ticks = now_ticks.or(self.last_ticks);
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

    /// A rejection followed by a recovery (stand retry, banked full pack)
    /// never fails a later park: the stale line stays past the baseline
    /// in the 25-line window, but the watch already consumed it. The live
    /// cook case: the walled west stand (3317,3138) refuses, the east
    /// stand cooks, then the bank heal parks 200+ ticks on the old line.
    #[test]
    fn recovered_rejection_never_re_arms() {
        let baseline = Value::Null;
        let mut watch = RejectWatch::new();
        let refused = (0, 3317, 3138);
        let recovered = (0, 3319, 3138);
        let bank = (0, 3269, 3164);
        assert!(watch.note(&show(&[("I can't reach that!", 7245)], 7245, refused), 7200, &baseline).is_none(), "arming poll never fails");
        assert!(watch.note(&show(&[("You put the shrimps on the stove.", 7250)], 7250, recovered), 7200, &baseline).is_none(), "moved bot resets");
        for tick in (7260..7330).step_by(5) {
            let state = show(&[("I can't reach that!", 7245), ("You put the shrimps on the stove.", 7250)], tick, bank);
            assert!(watch.note(&state, 7200, &baseline).is_none(), "stale refusal fails the bank park at tick {tick}");
        }
    }

    /// A strictly newer rejection arms again after a recovery: the grain
    /// pack fills twice (two bank trips), the second full-pack line is a
    /// new sighting on the current tile, not the consumed one.
    #[test]
    fn fresh_second_rejection_arms_again() {
        let baseline = Value::Null;
        let mut watch = RejectWatch::new();
        let field = (0, 3220, 3339);
        let bank = (0, 3251, 3420);
        assert!(watch.note(&show(&[("You can't carry any more grain.", 1886)], 1886, field), 1800, &baseline).is_none());
        assert!(watch.note(&show(&[("You open the bank.", 1900)], 1900, bank), 1800, &baseline).is_none(), "bank trip resets");
        assert!(watch.note(&show(&[("You open the bank.", 1900), ("You can't carry any more grain.", 2012)], 2012, bank), 1800, &baseline).is_none(), "new line only arms");
        let cause = watch.note(&show(&[("You open the bank.", 1900), ("You can't carry any more grain.", 2012)], 2032, bank), 1800, &baseline).expect("parked 20 ticks on a fresh refusal fails");
        assert!(cause.contains("carry"), "cause names the refusal: {cause}");
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
