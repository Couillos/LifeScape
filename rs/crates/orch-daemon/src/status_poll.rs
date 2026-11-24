#![forbid(unsafe_code)]
//! Lot 4.5: when the supervisor still asks a worker for its `status`.
//!
//! The heartbeat frames carry the phase and the status (lot 4.2) and prove
//! the worker lives, so the blocking `status` round trip is no longer a
//! poll of every bot every sweep. It is made when
//! - the bot has been silent for `SILENCE_SECS` (no frame: the worker may
//!   be frozen or reconnecting; the call, its 5 s bound and the circuit
//!   breaker decide), or never framed at all (an older worker),
//! - the frames show a phase or status other than the last reply's (a
//!   transition: the reply carries the error, the last code and the event
//!   a frame does not),
//! - `BACKSTOP_SECS` have passed since the last call (paused, held and
//!   script arguments only change through commands that apply their own
//!   reply, the backstop catches anything else).
//! A dead process needs no call at all (`Plan::Death`). The decision is a
//! pure function of the frame view and the last call, tested without a
//! worker; the frozen-worker kill stays in `daemon_watch` (frames older
//! than 45 s).

use lifescape_orch_fleet::jobs::FrameView;

/// A bot whose last frame is older than this is silent. The worker beats
/// at least once a second (`HEARTBEAT_WALL_SECS`, the idle loop sleeps
/// 1 s), 5 beats of margin, and far below the watchdog's 45 s.
pub const SILENCE_SECS: f64 = 5.0;
/// Longest time without a `status` reply for a bot that frames normally.
pub const BACKSTOP_SECS: f64 = 60.0;
/// Shortest gap between two calls on a transition (a frame that stays at
/// odds with the last reply never turns the call back into a per-sweep poll).
pub const TRANSITION_GAP_SECS: f64 = 2.0;
/// Shortest gap between two calls on a silent bot (the old pace: the
/// call's own bound is 5 s).
pub const SILENT_GAP_SECS: f64 = 5.0;

/// The last `status` reply of a bot, kept by the supervisor thread.
#[derive(Clone, Debug, PartialEq)]
pub struct LastPoll {
    pub at: f64,
    pub phase: String,
    pub status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Silent,
    Transition,
    Backstop,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::Silent => "status_silent",
            Reason::Transition => "status_transition",
            Reason::Backstop => "status_backstop",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The process tracked by the daemon is gone: handle the death.
    Death,
    /// Ask the worker for its status.
    Status(Reason),
    /// The frames say enough.
    Skip,
}

/// The supervisor's decision for one bot at `now` (daemon clock).
pub fn plan(tracked: bool, alive: bool, frame: Option<&FrameView>, last: Option<&LastPoll>, now: f64) -> Plan {
    if tracked && !alive {
        return Plan::Death;
    }
    let since_call = last.map(|poll| now - poll.at);
    let silent = frame.is_none_or(|view| now - view.received >= SILENCE_SECS);
    if silent {
        return if since_call.is_none_or(|gap| gap >= SILENT_GAP_SECS) { Plan::Status(Reason::Silent) } else { Plan::Skip };
    }
    if let (Some(view), Some(poll)) = (frame, last) {
        let moved = view.phase != poll.phase || view.status != poll.status;
        if moved && now - poll.at >= TRANSITION_GAP_SECS {
            return Plan::Status(Reason::Transition);
        }
    }
    match since_call {
        None => Plan::Status(Reason::Backstop),
        Some(gap) if gap >= BACKSTOP_SECS => Plan::Status(Reason::Backstop),
        Some(_) => Plan::Skip,
    }
}

/// Counters of the window (atomics: the supervisor thread counts, nobody
/// else reads but the summary).
static CALLS: [std::sync::atomic::AtomicU64; 3] = [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)];
static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WINDOW_SINCE: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);

/// Count a decision of the supervisor (deaths are logged by their handler).
pub fn count(decision: Plan) {
    use std::sync::atomic::Ordering::Relaxed;
    match decision {
        Plan::Status(Reason::Silent) => CALLS[0].fetch_add(1, Relaxed),
        Plan::Status(Reason::Transition) => CALLS[1].fetch_add(1, Relaxed),
        Plan::Status(Reason::Backstop) => CALLS[2].fetch_add(1, Relaxed),
        Plan::Skip => SKIPPED.fetch_add(1, Relaxed),
        Plan::Death => 0,
    };
}

/// `supervise status (60s): calls=N silent=.. transition=.. backstop=.. skipped=..`
/// once a minute (counters restart), `None` before or when nothing was decided.
pub fn summary_if_due(now: f64) -> Option<String> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut since = WINDOW_SINCE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if *since == 0.0 {
        *since = now;
    }
    if now - *since < 60.0 {
        return None;
    }
    let (silent, transition, backstop, skipped) = (CALLS[0].swap(0, Relaxed), CALLS[1].swap(0, Relaxed), CALLS[2].swap(0, Relaxed), SKIPPED.swap(0, Relaxed));
    let span = now - *since;
    *since = now;
    (silent + transition + backstop + skipped > 0).then(|| {
        format!("supervise status ({span:.0}s): calls={} silent={silent} transition={transition} backstop={backstop} skipped={skipped}", silent + transition + backstop)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(phase: &str, status: &str, received: f64) -> FrameView {
        FrameView { phase: phase.to_string(), status: status.to_string(), sent: received, received, seq: 1 }
    }

    fn poll(at: f64, phase: &str, status: &str) -> LastPoll {
        LastPoll { at, phase: phase.to_string(), status: status.to_string() }
    }

    /// Worker killed: the tracked process is dead, the death is handled
    /// whatever the frames or the last call say, and no call is made.
    #[test]
    fn a_killed_worker_is_a_death_not_a_call() {
        let fresh = view("running", "running", 99.9);
        assert_eq!(plan(true, false, Some(&fresh), Some(&poll(99.0, "running", "running")), 100.0), Plan::Death);
        assert_eq!(plan(true, false, None, None, 100.0), Plan::Death);
        // An adopted worker the daemon does not track is not declared dead.
        assert_ne!(plan(false, false, None, None, 100.0), Plan::Death);
    }

    /// Worker frozen: its frames stop, the call is made (once per 5 s) and
    /// its bound plus the circuit breaker take over; the kill itself is the
    /// watchdog's (`daemon_watch`, 45 s).
    #[test]
    fn a_frozen_worker_is_asked_after_the_silence_then_every_five_seconds() {
        let last_frame = view("running", "running", 100.0);
        let polled = poll(100.0, "running", "running");
        assert_eq!(plan(true, true, Some(&last_frame), Some(&polled), 104.9), Plan::Skip, "4.9 s is not silence");
        assert_eq!(plan(true, true, Some(&last_frame), Some(&polled), 105.0), Plan::Status(Reason::Silent), "5 s of silence, asked 5 s ago");
        let again = poll(105.1, "running", "running");
        assert_eq!(plan(true, true, Some(&last_frame), Some(&again), 108.0), Plan::Skip, "not before the 5 s gap");
        assert_eq!(plan(true, true, Some(&last_frame), Some(&again), 110.2), Plan::Status(Reason::Silent));
    }

    /// Worker reconnecting: frames keep coming with the status
    /// `reconnecting`: one call at the transition, then quiet; back to
    /// idle, one call again. Never a per-sweep poll.
    #[test]
    fn a_reconnecting_worker_costs_one_call_per_transition() {
        let mut last = poll(100.0, "running", "running");
        let reconnecting = view("running", "reconnecting", 103.0);
        assert_eq!(plan(true, true, Some(&reconnecting), Some(&last), 103.2), Plan::Status(Reason::Transition));
        last = poll(103.2, "running", "reconnecting");
        for step in 1..=20 {
            let now = 103.2 + f64::from(step) * 0.5;
            let beat = view("running", "reconnecting", now - 0.3);
            assert_eq!(plan(true, true, Some(&beat), Some(&last), now), Plan::Skip, "frames agree with the last reply at {now}");
        }
        let back = view("idle", "idle", 120.0);
        assert_eq!(plan(true, true, Some(&back), Some(&last), 120.2), Plan::Status(Reason::Transition));
    }

    /// A frame that stays at odds with the reply (a persistent divergence)
    /// is called at most once per gap, never every sweep.
    #[test]
    fn a_persistent_divergence_is_bounded_by_the_gap() {
        let last = poll(100.0, "idle", "idle");
        let frame = view("running", "running", 101.4);
        assert_eq!(plan(true, true, Some(&frame), Some(&last), 101.5), Plan::Skip, "inside the 2 s gap");
        assert_eq!(plan(true, true, Some(&view("running", "running", 102.0)), Some(&last), 102.1), Plan::Status(Reason::Transition));
    }

    /// Healthy bots are asked once a minute and on first contact only.
    #[test]
    fn healthy_bots_ride_the_backstop() {
        let frame = |now: f64| view("running", "running", now - 0.4);
        let last = poll(100.0, "running", "running");
        assert_eq!(plan(true, true, Some(&frame(130.0)), Some(&last), 130.0), Plan::Skip);
        assert_eq!(plan(true, true, Some(&frame(160.0)), Some(&last), 160.0), Plan::Status(Reason::Backstop));
        assert_eq!(plan(true, true, Some(&frame(10.0)), None, 10.0), Plan::Status(Reason::Backstop), "first contact");
        assert_eq!(plan(true, true, None, None, 10.0), Plan::Status(Reason::Silent), "an older worker never frames: the old poll, every 5 s");
    }

    /// Calls per bot-hour: from one per sweep (~10 s, 360) to transitions
    /// (a bot changes phase about 10 times an hour) plus the minute backstop.
    #[test]
    fn calls_per_account_hour_fall_several_fold() {
        let (mut calls, mut last) = (0u32, None::<LastPoll>);
        let (mut phase, mut now) = ("running", 0.0_f64);
        while now < 3600.0 {
            if (now as u64) % 360 == 0 && now > 0.0 {
                phase = if phase == "running" { "idle" } else { "running" };
            }
            let frame = view(phase, phase, now - 0.5);
            if let Plan::Status(_) = plan(true, true, Some(&frame), last.as_ref(), now) {
                calls += 1;
                last = Some(poll(now, phase, phase));
            }
            now += 0.5;
        }
        assert!(calls < 100, "{calls} calls per account-hour (10 transitions + the minute backstop) against ~360 at the old pace");
    }
}
