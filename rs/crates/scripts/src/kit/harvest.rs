//! Harvest: one click-and-watch core for every gather skill. Woodcut,
//! mining and fishing ran three copies of the same loop (click once, then
//! only re-click after a quiet round; a swing acked on a still-live target
//! means the player IS working, so keep waiting). What differs is the
//! target (decor loc vs NPC spot), the liveness proof and the give-up
//! rule — those are the `HarvestSource` particularités, implemented by
//! each activity. Approaches (walk preludes, stand rings) stay caller-side.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

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
