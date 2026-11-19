//! Market pauses inside a trip.
//!
//! The session pause gate (`session_rescue::gate_market`) runs a market
//! delivery inline, at the end of a pump: often the pump of a walk leg in
//! flight. Since 2026-11-19 the delivery stays at the Varrock square
//! (no walk back, the pack banked but the tools) and closes the pause
//! (`Bot::note_pause`); the trip it interrupted resumes from there.
//! Before, the delivery walked back to the tile where the pause began,
//! and the trip resumed on a stale state: its tick
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
//! dossier, its deadline pushed back by the pause length and its options
//! read again (the pause traded or banked the pack). Every closed pause
//! replans, however many: each replan needs a NEW closed pause, so the
//! loop cannot spin (live 2026-11-19: a bot at the Varrock square served
//! four deliveries inside one trip; the old bound of two wrote 14
//! `paused-mid-trip` dossiers an hour for trips that never failed).

#![forbid(unsafe_code)]

use lifescape_nav::plan::{NavOptions, NoRouteReason, Plan, Tile};

use super::travel::RemoteOutcome;
use crate::client::Bot;
use crate::error::Error;

/// Prefix of the marker a walk leg returns when a pause closed during its
/// wait: the leg stops at once instead of stalling for `STALL_TICKS`.
pub(crate) const PAUSED_MARK: &str = "paused-mid-trip";

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
        options: &mut NavOptions,
        deadline: &mut u64,
    ) -> TripRun {
        loop {
            let mark = self.pause_ticks;
            let segments = plan.segments.len().max(1);
            let (failed, err) = match self.run_plan(plan, segments, *deadline) {
                Ok(()) => return TripRun::Done,
                Err(failure) => failure,
            };
            if !pause_caused(&err, mark, self.pause_ticks) {
                return TripRun::Failed(failed, err);
            }
            let paused = self.pause_ticks.saturating_sub(mark);
            *deadline = deadline.saturating_add(paused);
            self.log(&format!(
                "nav pause: {paused} ticks of market pause during seg {}/{segments} ({err}), replan from {:?}",
                failed + 1,
                self.position(),
            ));
            let from = Tile { x: self.world.x, z: self.world.z, level: self.world.level };
            match self.pause_replan(options, from, dest) {
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

    /// Replan after a market pause from where it left the bot. The pause
    /// traded, banked or spent the pack (toll coins, fares, keys): the
    /// options are read again from live state; only the trip's own bans
    /// carry over (live 2026-11-19: toll gate planned with coins the
    /// pause had traded away, `DoorDidNotOpen`).
    pub(crate) fn pause_replan(&mut self, options: &mut NavOptions, from: Tile, dest: Tile) -> RemoteOutcome {
        let mut fresh = match self.nav_options_remote() {
            Ok(fresh) => fresh,
            Err(error) => return RemoteOutcome::NavDown(format!("options after pause: {error}")),
        };
        fresh.excluded_edges = std::mem::take(&mut options.excluded_edges);
        *options = fresh;
        self.plan_remote(options, from, dest)
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

    /// Live 2026-11-19 (kate_ellis-6031, lara_lane-9694, ella_quill-59680,
    /// "toll door loc 2882 ... left us at (3267,3227); last game: Accepted
    /// trade."): the trip planned the toll with 10+ coins in the pack, a
    /// market pause traded/banked them, and the pause replan reused the
    /// trip's stale options (`allow_paid`): the guard took no money, the
    /// gate never opened. The pause replan reads the pack again; the
    /// trip's own bans carry over.
    #[test]
    fn pause_replan_reads_the_pack_again() {
        use crate::client::travel::{test_nav_context, RemoteOutcome};
        use lifescape_nav::plan::{EdgeId, Tile};
        use lifescape_nav::service::{test_socket, PlanClient, Service, Snapshot};

        let (world, tables) = test_nav_context();
        let version = "pause-pack";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-pause-pack"), 1)
            .expect("service starts");
        let mut bot = crate::client::Bot::new("127.0.0.1", 43594, ".", ".");
        (bot.world.x, bot.world.z, bot.world.level) = (3267, 3227, 0);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "pause-pack"));
        bot.plan_data_version = Some(version.to_string());
        bot.allow_paid = true;
        const PACK: u16 = 3214;
        bot.world.inv.insert(PACK, std::collections::HashMap::from([(0u8, (995u16, 10u32)), (1u8, (1925u16, 1u32))]));
        let mut options = bot.nav_options_remote().expect("options");
        assert!(options.allow_paid, "10 coins in the pack: the toll is open");
        let ban = EdgeId::Ride("some_ladder".to_string());
        options.excluded_edges.insert(ban.clone());
        // The pause trades the coins away (the bucket stays).
        bot.world.inv.insert(PACK, std::collections::HashMap::from([(1u8, (1925u16, 1u32))]));
        let from = Tile { x: 3267, z: 3227, level: 0 };
        let dest = Tile { x: 3276, z: 3193, level: 0 };
        let plan = match bot.pause_replan(&mut options, from, dest) {
            RemoteOutcome::Ready(plan, _) => plan,
            _ => panic!("pause replan plans"),
        };
        assert!(!options.allow_paid, "no coins left: the toll is closed");
        assert!(options.excluded_edges.contains(&ban), "the trip's bans carry over");
        assert_eq!(lifescape_nav::fare::route_fare(&plan), 0, "no toll gate on the replanned route");
    }

    /// Live 2026-11-19 (14 `paused-mid-trip` dossiers in the first hour,
    /// ida_reed-10236: four deliveries of 500-800 ticks in one trip out
    /// of the Varrock square): every pause closed inside the trip replans
    /// it from where the pause left the bot, however many came. Here four
    /// pauses close on the first four leg waits, then the fake server
    /// never walks the bot: the trip ends on that stall, its own failure,
    /// never on the fourth pause's marker.
    #[test]
    fn every_closed_pause_replans_the_trip() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        use crate::client::travel::test_nav_context;
        use crate::isaac::Isaac;
        use lifescape_nav::plan::{NavOptions, Plan, Segment, Tile};
        use lifescape_nav::service::{test_socket, PlanClient, Service, Snapshot};

        let (world, tables) = test_nav_context();
        let version = "pause-every";
        let service = Service::start(Snapshot::new(world, tables, version.to_string()), &test_socket("sdk-pause-every"), 1)
            .expect("service starts");
        let (client_seed, server_seed) = ([11, 22, 33, 44], [55, 66, 77, 88]);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
        let port = listener.local_addr().expect("local addr").port();
        // Idle ticks (own player never moves) until the client hangs up.
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            sock.set_nonblocking(true).expect("nonblocking");
            let mut cipher = Isaac::new(server_seed);
            let mut sink = [0u8; 256];
            for _ in 0..4000 {
                let _ = sock.read(&mut sink);
                let frame = [167u8.wrapping_add(cipher.next_int() as u8), 0, 2, 0x00, 0x00];
                if sock.write_all(&frame).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
        });
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).expect("loopback dial");
        client.set_read_timeout(Some(std::time::Duration::from_millis(500))).expect("client read timeout");
        let mut bot = crate::client::Bot::new("127.0.0.1", port, ".", ".");
        bot.conn.attach_for_test(client, client_seed, server_seed);
        bot.last_keepalive = Some(std::time::Instant::now());
        bot.last_send_tick = bot.world.ticks;
        (bot.world.x, bot.world.z, bot.world.level) = (3222, 3218, 0);
        bot.plan_client = Some(PlanClient::new(service.socket_path(), "pause-every"));
        bot.plan_data_version = Some(version.to_string());
        let gate_calls = Arc::new(AtomicU64::new(0));
        let calls = Arc::clone(&gate_calls);
        bot.pause_gate = Some(Arc::new(move |bot: &mut crate::client::Bot| {
            // One pause per leg wait, four times: closed at once.
            if bot.pauses_done < 4 && calls.fetch_add(1, Ordering::SeqCst) < 400 {
                bot.note_pause(10);
            }
            true
        }));
        let dest = Tile { x: 3224, z: 3218, level: 0 };
        let mut plan = Plan {
            segments: vec![Segment::Walk { tiles: vec![Tile { x: 3223, z: 3218, level: 0 }, dest] }],
            cost: 2,
            adjusted_dest: None,
            dest_blocker: None,
        };
        let mut options = NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: std::collections::HashSet::new(),
            excluded_edges: std::collections::HashSet::new(),
            danger: Vec::new(),
        };
        let mut deadline = bot.world.ticks + 400;
        let outcome = bot.run_plan_through_pauses(&mut plan, dest, &mut options, &mut deadline);
        let pauses = bot.pauses_done;
        drop(bot);
        let _ = server.join();
        assert_eq!(pauses, 4, "four deliveries closed inside the trip");
        match outcome {
            super::TripRun::Failed(_, error) => {
                assert!(!is_paused_mark(&error), "the trip ends on its own stall, never on a pause: {error:?}")
            }
            super::TripRun::Done => panic!("the fake server never walks the bot"),
            super::TripRun::Final(error) => panic!("replans plan: {error:?}"),
        }
    }

    /// The leg marker is recognised by prefix only.
    #[test]
    fn paused_mark_is_recognised() {
        assert!(is_paused_mark(&Error::StepRefused(format!("{PAUSED_MARK} at (0, 1, 2)"))));
        assert!(!is_paused_mark(&Error::StepRefused("step refused".to_string())));
        assert!(!is_paused_mark(&Error::Timeout(PAUSED_MARK.to_string())));
    }
}
