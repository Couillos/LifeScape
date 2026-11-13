//! Market pauses inside a trip.
//!
//! The session pause gate (`session_rescue::gate_market`) runs a market
//! delivery inline, at the end of a pump: often the pump of a walk leg in
//! flight. The delivery walks to the bank and the square, then walks back
//! to the tile where the pause began and closes it (`Bot::note_pause`).
//! The trip it interrupted then resumes on a stale state: its tick
//! deadline counted the whole pause (400-900 ticks live), the server walk
//! queue was replaced, and the door registry was cleared by the nested
//! trip. Live 2026-11-13 (21:19-21:58 CET): 28 `walk timed out` / `door loc
//! timed out`, ~30 `step refused` and 5 `door did not open` dossiers sat
//! inside a market pause.
//!
//! The executor reads the closed-pause counter (`pause_ticks`, advanced by
//! the gate only) around each plan run: a segment failure with a pause
//! closed meanwhile is the pause's doing, never the plan's. The trip
//! replans from where the pause left the bot, with no edge banned, no
//! dossier, and its deadline pushed back by the pause length. Bounded
//! (`PAUSE_REPLANS`): a third pause in one trip fails like any failure.

#![forbid(unsafe_code)]

use lifescape_nav::plan::{NavOptions, NoRouteReason, Plan, Tile};

use super::travel::RemoteOutcome;
use crate::client::Bot;
use crate::error::Error;

/// Prefix of the marker a walk leg returns when a pause closed during its
/// wait: the leg stops at once instead of stalling for `STALL_TICKS`.
pub(crate) const PAUSED_MARK: &str = "paused-mid-trip";

/// Pause replans one trip may take (deliveries are minutes apart; two
/// covers a long trip, a third pause is a real anomaly).
pub(crate) const PAUSE_REPLANS: u32 = 2;

/// One plan run through the pause guard.
pub(crate) enum TripRun {
    /// Every segment held.
    Done,
    /// A segment failed on its own: the caller's replan/dossier path.
    Failed(usize, Error),
    /// The pause replan itself ended the trip (no route from where the
    /// pause left us, nav down, stop): already typed, already reported.
    Final(Error),
}

/// True when a segment failure is the pause's doing: a pause closed since
/// `mark` (closed-pause counter before the run) and the failure is one a
/// moved bot or a spent deadline produces. Stops, socket failures, random
/// events and choice dialogues keep their own meaning.
pub(crate) fn pause_caused(error: &Error, mark: u64, now: u64) -> bool {
    now != mark
        && matches!(
            error,
            Error::Timeout(_)
                | Error::StepRefused(_)
                | Error::DoorDidNotOpen(_)
                | Error::RideDidNotMove(_)
                | Error::RideWrongDestination(_)
        )
}

/// True for the marker a walk leg returns after a pause.
pub(crate) fn is_paused_mark(error: &Error) -> bool {
    matches!(error, Error::StepRefused(detail) if detail.starts_with(PAUSED_MARK))
}

impl Bot {
    /// Marker for a leg whose wait hosted a market pause.
    pub(crate) fn paused_mark(&self) -> Error {
        Error::StepRefused(format!("{PAUSED_MARK} at {:?}", self.position()))
    }

    /// Run `plan`; a failure caused by a market pause replans from the
    /// current position (no ban, no dossier) and runs again, the
    /// deadline pushed back by the pause. `plan` ends as the last plan
    /// run, so the caller's ban and dossier name the right segment.
    pub(crate) fn run_plan_through_pauses(
        &mut self,
        plan: &mut Plan,
        dest: Tile,
        options: &NavOptions,
        deadline: &mut u64,
        budget: &mut u32,
    ) -> TripRun {
        loop {
            let mark = self.pause_ticks;
            let segments = plan.segments.len().max(1);
            let (failed, err) = match self.run_plan(plan, segments, *deadline) {
                Ok(()) => return TripRun::Done,
                Err(failure) => failure,
            };
            if *budget == 0 || !pause_caused(&err, mark, self.pause_ticks) {
                return TripRun::Failed(failed, err);
            }
            *budget -= 1;
            let paused = self.pause_ticks.saturating_sub(mark);
            *deadline = deadline.saturating_add(paused);
            self.log(&format!(
                "nav pause: {paused} ticks of market pause during seg {}/{segments} ({err}), replan from {:?}",
                failed + 1,
                self.position(),
            ));
            let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
            match self.plan_remote(options, from, dest) {
                RemoteOutcome::Ready(next, _) => {
                    self.publish_plan(&next, dest);
                    *plan = next;
                }
                RemoteOutcome::NoRoute(no) if no.reason == NoRouteReason::Aborted => {
                    return TripRun::Final(Error::Interrupted(format!("replan aborted at {:?}", self.position())));
                }
                RemoteOutcome::NoRoute(no) => return TripRun::Final(self.fail_no_route(from, dest, options, no)),
                RemoteOutcome::Interrupted(message) => return TripRun::Final(Error::Interrupted(message)),
                RemoteOutcome::NavDown(reason) => return TripRun::Final(Error::Travel(format!("nav-down: {reason}"))),
                RemoteOutcome::Session(error) => return TripRun::Final(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_paused_mark, pause_caused, PAUSED_MARK};
    use crate::error::Error;

    /// Live 2026-11-13 (bcm9kxn4-43018): a 427-tick market pause inside
    /// a 1-tile walk read `walk timed out`. A closed pause turns the
    /// executor failures into a pause replan; stops and socket errors
    /// never do, and nothing converts without a pause.
    #[test]
    fn only_a_closed_pause_explains_a_failure() {
        let timeout = Error::Timeout("walk timed out at (0, 3237, 3406)".to_string());
        assert!(pause_caused(&timeout, 100, 527), "pause closed mid-trip: replan");
        assert!(!pause_caused(&timeout, 100, 100), "no pause: the failure stands");
        for kept in [
            Error::Interrupted("run stopped".to_string()),
            Error::Io("socket".to_string()),
            Error::Login("relog".to_string()),
            Error::Unreachable("random event between legs".to_string()),
            Error::NeedsDialogueChoice("menu".to_string()),
        ] {
            assert!(!pause_caused(&kept, 100, 527), "{kept:?} keeps its meaning");
        }
        for converted in [
            Error::StepRefused("step refused".to_string()),
            Error::DoorDidNotOpen("door loc 1530 never swung".to_string()),
            Error::RideDidNotMove("ladder".to_string()),
            Error::RideWrongDestination("ladder".to_string()),
        ] {
            assert!(pause_caused(&converted, 100, 527), "{converted:?} after a pause replans");
        }
    }

    /// Live 2026-11-13 (bcm9kxn4-40523, bcm9kxn4-32645, nina_rhodes-26612):
    /// a pause began while the bot finished a door crossing step; the
    /// delivery planned from the stale stand, and its first segment, that
    /// same door, found the bot already on `cross_to`: "off stand", a
    /// dossier, a replan. On `cross_to` the door segment already holds.
    #[test]
    fn door_already_crossed_holds() {
        use lifescape_nav::plan::{DoorRef, Tile};
        let mut bot = crate::client::Bot::new("127.0.0.1", 43594, ".", ".");
        (bot.world.x, bot.world.z, bot.world.level) = (3241, 3406, 0);
        let door = DoorRef { loc: 1530, x: 3241, z: 3406, level: 0, open_op: Some(1), toll: false };
        let outcome = bot.exec_door(&door, Tile { x: 3240, z: 3406, level: 0 }, Tile { x: 3241, z: 3406, level: 0 }, 1000);
        assert!(outcome.is_ok(), "already on cross_to: the crossing holds, got {outcome:?}");
        (bot.world.x, bot.world.z) = (3242, 3407);
        let off = bot.exec_door(&door, Tile { x: 3240, z: 3406, level: 0 }, Tile { x: 3241, z: 3406, level: 0 }, 1000);
        assert!(matches!(off, Err(Error::StepRefused(_))), "elsewhere stays off stand, got {off:?}");
    }

    /// The leg marker is recognised by prefix only.
    #[test]
    fn paused_mark_is_recognised() {
        assert!(is_paused_mark(&Error::StepRefused(format!("{PAUSED_MARK} at (0, 1, 2)"))));
        assert!(!is_paused_mark(&Error::StepRefused("step refused".to_string())));
        assert!(!is_paused_mark(&Error::Timeout(PAUSED_MARK.to_string())));
    }
}
