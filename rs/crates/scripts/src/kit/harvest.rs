//! Harvest: one click-and-watch core for every gather skill. Woodcut,
//! mining and fishing ran three copies of the same loop (click once, then
//! only re-click after a quiet round; a swing acked on a still-live target
//! means the player IS working, so keep waiting). What differs is the
//! target (decor loc vs NPC spot), the liveness proof and the give-up
//! rule — those are the `HarvestSource` particularités, implemented by
//! each activity. Approaches (walk preludes, stand rings) stay caller-side.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::trees::resolve_loc;
use crate::skill_common::{check_events, dismiss_modal, hitpoints, WorkOutcome};
use crate::skill_gather::{classify_message, work_acked};
use crate::ScriptError;

/// One quiet round: the server chains attempts itself while in range, so
/// a click opens the work and quiet rounds only probe. Shared by all
/// three gather loops.
pub const ROUND_IDLE_TICKS: u64 = 12;

/// What a click opened: a decor loc id (woodcut, mining) or an NPC slot
/// (fishing). Liveness checks match against it.
pub enum HarvestTarget {
    Loc { loc: u16 },
    Npc { slot: u16 },
}

/// Per-skill particularités of the click-and-watch loop.
pub trait HarvestSource {
    /// Server start-of-work acks for this skill (`WOODCUT_ACKS` & co).
    fn acks(&self) -> &[&str];
    /// Click the resource at (x, z). Logs the click line itself.
    fn click(&mut self, bot: &mut Bot, x: i32, z: i32) -> Result<HarvestTarget, ScriptError>;
    /// The NPC spot vanished from the stream (fishing: spot moved off).
    /// Decor targets never vanish this way (a cleared loc streams).
    fn slot_gone(&self, bot: &mut Bot, target: &HarvestTarget) -> bool;
    /// The NPC spot drifted away from the clicked tile (fishing: >6).
    /// Decor targets never drift.
    fn slot_moved(&self, bot: &mut Bot, target: &HarvestTarget) -> bool;
    /// The clicked target is still the live resource (loc id match,
    /// unstumped tile, present NPC slot).
    fn target_live(&self, bot: &mut Bot, target: &HarvestTarget) -> bool;
    /// Fast depletion path on a quiet round (woodcut: stump on the first
    /// silence). Mining and fishing wait out the quiet rounds instead.
    fn depleted_now(&self, bot: &mut Bot, x: i32, z: i32, silences: u32) -> bool;
    /// Quiet rounds before giving up without any ack (woodcut/mining 2,
    /// fishing ~5: its 60-tick unacked rule in 12-tick rounds).
    fn unacked_rounds(&self) -> u32 {
        2
    }
    /// A gain resets the quiet count (fishing: its 60-tick rule counts
    /// from the last fish, not from the click).
    fn silence_resets_on_gain(&self) -> bool {
        false
    }
    /// Re-click decision after a quiet round (woodcut: never on an acked
    /// live tree — a re-click restarts the swing; mining: always, as a
    /// reachability probe; fishing: every ~30 ticks while acked).
    fn reprobe(&self, acked: bool, live: bool, since_probe: u64) -> bool;
    /// Random events are polled per quiet round (woodcut; the others poll
    /// at the loop level instead).
    fn poll_events(&self) -> bool {
        false
    }
    /// Dry-streak note on an acked live target (woodcut, fishing; mining
    /// stays quiet). Called with the round count so each skill keeps its
    /// own cadence.
    fn note_dry_streak(&mut self, _bot: &mut Bot, _x: i32, _z: i32, _acked: bool, _live: bool, _silences: u32) {}
}

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range). A swing acked on a still-live target
/// means the player IS working — keep waiting; the load-level stall stays
/// the ultimate backstop. Gained product this stand. Pure delegation for
/// everything skill-specific (`HarvestSource`); threats, modals and the
/// refusal vocabulary are shared.
pub fn watch_stand<Source: HarvestSource>(
    bot: &mut Bot,
    source: &mut Source,
    spot_x: i32,
    spot_z: i32,
    product_ids: &[u16],
    had: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    let mut gained = 0u32;
    let mut silences = 0u32;
    let mut acked = false;
    let mut need_click = true;
    let mut target: Option<HarvestTarget> = None;
    let mut seen_msgs = bot.world.messages.len();
    let mut last_probe = bot.world.ticks;
    let hitpoints_before = hitpoints(bot);
    loop {
        if need_click {
            target = Some(source.click(bot, spot_x, spot_z)?);
            seen_msgs = bot.world.messages.len();
            last_probe = bot.world.ticks;
            need_click = false;
        }
        let round_target = bot.world.ticks + ROUND_IDLE_TICKS;
        let mut quiet = true;
        while bot.world.ticks < round_target {
            bot.pump(2)?;
            if hitpoints(bot) < hitpoints_before {
                return Ok((WorkOutcome::Threat, gained));
            }
            if bot.world.modal.is_some() && !dismiss_modal(bot)? {
                bot.pump(10)?;
                if bot.world.modal.is_some() && !dismiss_modal(bot)? {
                    return Ok((WorkOutcome::Blocked, gained));
                }
            }
            for message in bot.world.messages.get(seen_msgs..).unwrap_or(&[]) {
                if let Some(outcome) = classify_message(message) {
                    return Ok((outcome, gained));
                }
            }
            if let Some(ref clicked) = target {
                if source.slot_gone(bot, clicked) {
                    return Ok((WorkOutcome::Gone, gained));
                }
                if source.slot_moved(bot, clicked) {
                    return Ok((WorkOutcome::Moved, gained));
                }
            }
            let now_have: u32 =
                product_ids.iter().map(|id| crate::skill_common::pack_total(bot, *id)).sum();
            if now_have > had + gained {
                gained = now_have - had;
                quiet = false;
                break;
            }
        }
        if !quiet {
            if source.silence_resets_on_gain() {
                silences = 0;
            }
            continue;
        }
        if source.poll_events() {
            check_events(bot)?;
        }
        silences += 1;
        if source.depleted_now(bot, spot_x, spot_z, silences) {
            return Ok((WorkOutcome::Gone, gained));
        }
        acked = acked || work_acked(bot, seen_msgs, source.acks());
        let live = match target {
            Some(ref clicked) => source.target_live(bot, clicked),
            None => false,
        };
        let limit = if acked && live { u32::MAX } else { source.unacked_rounds() };
        if silences >= limit {
            return Ok((WorkOutcome::Gone, gained));
        }
        source.note_dry_streak(bot, spot_x, spot_z, acked, live, silences);
        need_click = source.reprobe(acked, live, bot.world.ticks.saturating_sub(last_probe));
    }
}

/// Live footprint (width, length) of the loc on this tile. The server
/// reaches shape-10 locs orthogonally to their footprint (`ReachStrategy`
/// `reachRectangle1`, engine `routefinder/ReachStrategy.ts`), never
/// diagonally to the anchor tile: yews and oaks are 3x3, most trees 2x2
/// (`gametypes.json` loc rows). A diagonal-to-anchor stand touches no
/// footprint edge and the click always refuses — live 6/6: (3150,3229)
/// fails on (3151,3230) while (3151,3229) cuts, (3183,3225) fails on
/// (3184,3226) while (3185,3225) cuts, (3164,3220) cuts (3165,3219).
/// Unknown loc reads as 1x1 (old behaviour, engine arbitrates on click).
pub(crate) fn spot_footprint(bot: &mut Bot, spot_x: i32, spot_z: i32) -> (i64, i64) {
    let footprint = resolve_loc(bot, spot_x, spot_z).ok().and_then(|(loc, angle)| {
        knowledge::loc_by_id(usize::from(loc)).map(|entry| {
            if angle % 2 == 1 {
                (entry.length, entry.width)
            } else {
                (entry.width, entry.length)
            }
        })
    });
    footprint.unwrap_or((1, 1))
}

/// True when the stand is orthogonally adjacent to the loc footprint, i.e.
/// the server can reach it: same geometry as `reachRectangle1` minus the
/// wall-flag checks the engine arbitrates on click. Chebyshev-to-anchor is
/// NOT the rule (a 3x3 corner stand is "adjacent" but inoperable).
pub(crate) fn operable_stand(bot_x: i32, bot_z: i32, spot_x: i32, spot_z: i32, width: i64, length: i64) -> bool {
    let (anchor_x, anchor_z) = (i64::from(spot_x), i64::from(spot_z));
    let (stand_x, stand_z) = (i64::from(bot_x), i64::from(bot_z));
    (stand_x == anchor_x - 1 && stand_z >= anchor_z && stand_z < anchor_z + length)
        || (stand_x == anchor_x + width && stand_z >= anchor_z && stand_z < anchor_z + length)
        || (stand_z == anchor_z - 1 && stand_x >= anchor_x && stand_x < anchor_x + width)
        || (stand_z == anchor_z + length && stand_x >= anchor_x && stand_x < anchor_x + width)
}

/// One stand per footprint side (side middles): the click rotation tries
/// these before condemning the tree. Sorted nearest-first by the caller.
pub(crate) fn operable_ring(spot_x: i32, spot_z: i32, width: i64, length: i64) -> Vec<(i32, i32)> {
    let mut ring = vec![
        (spot_x - 1, spot_z + (length as i32) / 2),
        (spot_x + width as i32, spot_z + (length as i32) / 2),
        (spot_x + (width as i32) / 2, spot_z - 1),
        (spot_x + (width as i32) / 2, spot_z + length as i32),
    ];
    ring.sort();
    ring.dedup();
    ring
}

#[cfg(test)]
mod tests {
    use super::{operable_ring, operable_stand};

    fn operable_stand_matches_server_reach() {
        // Yew 3x3 at (3151,3230): south side works, west corner refuses.
        assert!(operable_stand(3151, 3229, 3151, 3230, 3, 3), "south side cuts");
        assert!(!operable_stand(3150, 3229, 3151, 3230, 3, 3), "diagonal corner refuses");
        // Yew 3x3 at (3184,3226): east-diagonal works, west-diagonal refuses.
        assert!(operable_stand(3185, 3225, 3184, 3226, 3, 3), "south-east diagonal works");
        assert!(!operable_stand(3183, 3225, 3184, 3226, 3, 3), "south-west corner refuses");
        // Yew 3x3 at (3165,3219): west side and south side both cut.
        assert!(operable_stand(3164, 3220, 3165, 3219, 3, 3), "west side cuts");
        assert!(operable_stand(3166, 3218, 3165, 3219, 3, 3), "south side cuts");
        // Willow 2x2: side middles cut, corners refuse, far tiles refuse.
        assert!(operable_stand(9, 10, 10, 10, 2, 2), "west side cuts");
        assert!(!operable_stand(9, 9, 10, 10, 2, 2), "corner refuses");
        assert!(!operable_stand(13, 11, 10, 10, 2, 2), "two tiles off refuses");
        // 1x1 keeps the old adjacency (all four orthogonals, no diagonals).
        assert!(operable_stand(5, 4, 5, 5, 1, 1), "south cuts");
        assert!(!operable_stand(4, 4, 5, 5, 1, 1), "diagonal refuses");
    }

    fn operable_ring_covers_four_sides() {
        let mut ring = operable_ring(3151, 3230, 3, 3);
        ring.sort();
        assert_eq!(ring, vec![(3150, 3231), (3152, 3229), (3152, 3233), (3154, 3231)], "side middles, got: {ring:?}");
        let mut single = operable_ring(5, 5, 1, 1);
        single.sort();
        assert_eq!(single, vec![(4, 5), (5, 4), (5, 6), (6, 5)], "four orthogonals, got: {single:?}");
    }}
