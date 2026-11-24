#![forbid(unsafe_code)]
//! Lot 4.2: silent comparison of the phase and status the heartbeat frames
//! carry against what the `status` call answers.
//!
//! The frames already reach the daemon every second; `Job::note_frame`
//! applies their phase and status. The supervisor's `status` call stays
//! (it carries more than the frames do) and, before applying its reply,
//! asks `observe` whether the frames said the same. Nothing is decided
//! here: counters, a window summary line every minute and a capped trace of
//! the divergences, so one live hour proves the frames can replace the call
//! (lot 4.5).

use std::sync::Mutex;

use lifescape_json::Value;
use lifescape_orch_fleet::jobs::FrameView;

/// A frame older than this before the call says nothing about it (an idle
/// worker beats about once a second, a wedged one not at all).
pub const STALE_AFTER_SECS: f64 = 5.0;
/// A divergence on a frame younger than this is a plausible transition
/// between the frame and the reply (one beat); older is a real mismatch.
pub const YOUNG_FRAME_SECS: f64 = 1.5;
/// Window of the summary line.
pub const SUMMARY_EVERY_SECS: f64 = 60.0;
/// Divergences traced one by one per window.
const TRACE_PER_WINDOW: u32 = 20;

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// No frame seen yet (fresh worker or an older one without `phase`).
    NoFrame,
    /// A frame landed while the call ran: either answer may be right.
    Raced,
    /// The last frame was too old to compare.
    Stale,
    Same,
    Diverged { phase: bool, status: bool, age: f64 },
}

/// Pure verdict: `before`/`after` are the frame views read around the
/// call, `reply` is the `status` session, `call_started` the daemon clock
/// when the call began.
pub fn judge(before: Option<&FrameView>, after: Option<&FrameView>, reply: &Value, call_started: f64) -> Verdict {
    let Some(after) = after else { return Verdict::NoFrame };
    if before.map(|view| view.seq) != Some(after.seq) {
        return Verdict::Raced;
    }
    let age = call_started - after.received;
    if age > STALE_AFTER_SECS {
        return Verdict::Stale;
    }
    let phase = reply.get("phase").as_str().is_some_and(|phase| phase != after.phase);
    let status = reply.get("status").as_str().is_some_and(|status| status != after.status);
    if phase || status {
        Verdict::Diverged { phase, status, age }
    } else {
        Verdict::Same
    }
}

#[derive(Default)]
struct Window {
    since: f64,
    same: u64,
    diverged_young: u64,
    diverged_old: u64,
    phase: u64,
    status: u64,
    raced: u64,
    stale: u64,
    none: u64,
    traced: u32,
}

impl Window {
    fn compared(&self) -> u64 {
        self.same + self.diverged_young + self.diverged_old
    }

    fn line(&self, now: f64) -> String {
        format!(
            "supervise frames ({:.0}s): compared={} same={} diverged={} (young={} old={} phase={} status={}) raced={} stale={} no_frame={}",
            now - self.since,
            self.compared(),
            self.same,
            self.diverged_young + self.diverged_old,
            self.diverged_young,
            self.diverged_old,
            self.phase,
            self.status,
            self.raced,
            self.stale,
            self.none,
        )
    }
}

fn window() -> &'static Mutex<Window> {
    static WINDOW: std::sync::OnceLock<Mutex<Window>> = std::sync::OnceLock::new();
    WINDOW.get_or_init(|| Mutex::new(Window::default()))
}

/// Count one comparison (and trace a divergence, capped per window).
pub fn observe(account: &str, before: Option<&FrameView>, after: Option<&FrameView>, reply: &Value, call_started: f64, now: f64) {
    let verdict = judge(before, after, reply, call_started);
    let mut window = window().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if window.since == 0.0 {
        window.since = now;
    }
    match &verdict {
        Verdict::NoFrame => window.none += 1,
        Verdict::Raced => window.raced += 1,
        Verdict::Stale => window.stale += 1,
        Verdict::Same => window.same += 1,
        Verdict::Diverged { phase, status, age } => {
            if *age < YOUNG_FRAME_SECS {
                window.diverged_young += 1;
            } else {
                window.diverged_old += 1;
            }
            window.phase += u64::from(*phase);
            window.status += u64::from(*status);
            if window.traced < TRACE_PER_WINDOW {
                window.traced += 1;
                let seen = after.map(|view| format!("phase={} status={}", view.phase, view.status)).unwrap_or_default();
                crate::logln!(
                    "supervise {account}: frame/status divergence: frame {seen} ({age:.1}s old), status call phase={} status={}",
                    reply.get("phase").as_str().unwrap_or("?"),
                    reply.get("status").as_str().unwrap_or("?"),
                );
            }
        }
    }
}

/// The window's summary line once `SUMMARY_EVERY_SECS` have passed (the
/// window then restarts), `None` before or when nothing was compared.
pub fn summary_if_due(now: f64) -> Option<String> {
    let mut window = window().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if window.since == 0.0 || now - window.since < SUMMARY_EVERY_SECS {
        return None;
    }
    let seen = window.compared() + window.raced + window.stale + window.none;
    let line = (seen > 0).then(|| window.line(now));
    *window = Window { since: now, ..Window::default() };
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json::{object, str_value};

    fn view(seq: u64, phase: &str, status: &str, received: f64) -> FrameView {
        FrameView { phase: phase.to_string(), status: status.to_string(), sent: received, received, seq }
    }

    fn reply(phase: &str, status: &str) -> Value {
        object(vec![("phase", str_value(phase)), ("status", str_value(status))])
    }

    #[test]
    fn verdicts() {
        let seen = view(4, "running", "running", 99.5);
        assert_eq!(judge(None, None, &reply("idle", "idle"), 100.0), Verdict::NoFrame);
        assert_eq!(judge(Some(&seen), Some(&seen), &reply("running", "running"), 100.0), Verdict::Same);
        assert_eq!(
            judge(Some(&seen), Some(&seen), &reply("idle", "running"), 100.0),
            Verdict::Diverged { phase: true, status: false, age: 0.5 }
        );
        assert_eq!(
            judge(Some(&seen), Some(&seen), &reply("running", "finished"), 100.0),
            Verdict::Diverged { phase: false, status: true, age: 0.5 }
        );
        let newer = view(5, "idle", "idle", 100.1);
        assert_eq!(judge(Some(&seen), Some(&newer), &reply("idle", "idle"), 100.0), Verdict::Raced);
        assert_eq!(judge(None, Some(&newer), &reply("idle", "idle"), 100.0), Verdict::Raced, "first frame landed during the call");
        let old = view(4, "running", "running", 90.0);
        assert_eq!(judge(Some(&old), Some(&old), &reply("idle", "idle"), 100.0), Verdict::Stale);
    }

    #[test]
    fn the_window_counts_traces_and_restarts() {
        let seen = view(1, "running", "running", 10.0);
        observe("t01", Some(&seen), Some(&seen), &reply("running", "running"), 10.2, 10.2);
        observe("t01", Some(&seen), Some(&seen), &reply("idle", "idle"), 10.4, 10.4);
        observe("t01", None, None, &reply("idle", "idle"), 10.5, 10.5);
        assert_eq!(summary_if_due(20.0), None, "a minute has not passed");
        let line = summary_if_due(71.0).expect("window closes");
        assert!(line.contains("compared=2 same=1 diverged=1 (young=1 old=0 phase=1 status=1) raced=0 stale=0 no_frame=1"), "{line}");
        assert_eq!(summary_if_due(200.0), None, "nothing compared since the restart");
    }
}
