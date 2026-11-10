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
use crate::kit::survival::{check_events, hitpoints};
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
                product_ids.iter().map(|id| crate::kit::pack::pack_total(bot, *id)).sum();
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


/// Spot work result, shared by the three gathering scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkOutcome {
    Full,
    Gone,
    Moved,
    NoTool,
    NoBait,
    Threat,
    Blocked,
    LevelLocked,
    Unreachable,
    /// Direct spot refusal: the server answered the chop/mine click itself
    /// with "can't reach that". Walk-leg failures (never arrived, leg
    /// timeout, step refused far away) stay plain `Unreachable`: only the
    /// gather loop's classifier may promote this to a long ban, on a
    /// repeated adjacent refusal — never on transit alone.
    UnreachableSpot,
}

/// Server refusal lines shared by the three gather clicks: direct refusals
/// ("can't reach that"), level gates, missing tools, missing bait, the
/// full-pack lines ("too full to hold any more", "can't carry any more"),
/// and rock depletion ("no ore/minerals currently available" reads Gone:
/// the pick repicks instead of waiting on an empty rock). `None`
/// when the line is chatter or progress (gain and combat engagement stay
/// caller-side; combat keeps its own under-attack parser).
pub fn classify_message(line: &str) -> Option<WorkOutcome> {
    let lowered = line.to_lowercase();
    if lowered.contains("can't reach that") {
        return Some(WorkOutcome::UnreachableSpot);
    }
    if lowered.contains("too full to hold any more") || lowered.contains("can't carry any more") {
        return Some(WorkOutcome::Full);
    }
    if lowered.contains("no ore currently available") || lowered.contains("no minerals currently available") {
        return Some(WorkOutcome::Gone);
    }
    if lowered.contains("level of") || (lowered.contains("need at least") && lowered.contains("fishing")) {
        return Some(WorkOutcome::LevelLocked);
    }
    if lowered.contains("do not have an axe") || lowered.contains("do not have a pickaxe") {
        return Some(WorkOutcome::NoTool);
    }
    if lowered.contains("need a ")
        && (lowered.contains("to catch") || lowered.contains("to bait") || lowered.contains("to lure"))
    {
        return Some(WorkOutcome::NoTool);
    }
    if lowered.contains("don't have any") && lowered.contains("left") {
        return Some(WorkOutcome::NoBait);
    }
    None
}

/// True when a work-start ack arrived since `seen` (the message count
/// right after the click). Case-insensitive substring, like the
/// refusal vocabulary in `classify_message`.
pub fn work_acked(bot: &Bot, seen: usize, acks: &[&str]) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|line| {
        let lowered = line.to_lowercase();
        acks.iter().any(|ack| lowered.contains(ack))
    })
}

/// Dismiss a level-up / dialog modal with Continue. True when no modal
/// remains; congratulation dialogs are the only ones auto-dismissed.
pub fn dismiss_modal(bot: &mut Bot) -> Result<bool, ScriptError> {
    if bot.world.modal.is_none() {
        return Ok(true);
    }
    // Overlay-layer modals (wilderness overlay 193) are informational: the
    // server keeps answering world clicks while one is open (live: op_loc
    // answered "I can't reach that!" with the overlay up), so there is
    // nothing to dismiss and the gather loops must not read it as Blocked.
    if bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "overlay") {
        return Ok(true);
    }
    let congrats = bot.chat_texts().iter().any(|(_, text)| text.to_lowercase().contains("congratulations"));
    if !congrats {
        return Ok(false);
    }
    for _ in 0..4 {
        bot.chat_continue()?;
        bot.pump(5)?;
        if bot.world.modal.is_none() {
            bot.log("ok dialog dismissed");
            return Ok(true);
        }
    }
    let target = bot.world.ticks + 25;
    while bot.world.ticks < target && bot.world.modal.is_some() {
        bot.pump(5)?;
    }
    Ok(bot.world.modal.is_none())

}

#[cfg(test)]
mod tests {
    use super::{classify_message, dismiss_modal, operable_ring, operable_stand, work_acked, WorkOutcome};
    use crate::kit::{fish::FISHING_ACKS, mine::MINING_ACKS, woodcut::WOODCUT_ACKS};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Operable-stand geometry mirrors the server (`ReachStrategy`
    /// `reachRectangle1`): orthogonal to the footprint, never diagonal to
    /// the anchor. Live 6/6 fixtures: a yew is 3x3, so (3151,3229) cuts
    /// (3151,3230) but the diagonal corner (3150,3229) refuses; (3185,3225)
    /// cuts (3184,3226) but (3183,3225) refuses; (3164,3220) and
    /// (3166,3218) both cut (3165,3219) (west side, inside the footprint
    /// edge range).
    #[test]
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

    /// Ring: one stand per footprint side, deduped (a 3x3 yields 4
    /// distinct side middles, a 1x1 the four orthogonals).
    #[test]
    fn operable_ring_covers_four_sides() {
        let mut ring = operable_ring(3151, 3230, 3, 3);
        ring.sort();
        assert_eq!(ring, vec![(3150, 3231), (3152, 3229), (3152, 3233), (3154, 3231)], "side middles, got: {ring:?}");
        let mut single = operable_ring(5, 5, 1, 1);
        single.sort();
        assert_eq!(single, vec![(4, 5), (5, 4), (5, 6), (6, 5)], "four orthogonals, got: {single:?}");
    }

    /// Message classes: direct refusals, level gates, missing tools/bait,
    /// full packs and rock depletion map; loot chatter stays `None`.
    #[test]
    fn classify_covers_the_gather_refusals() {
        assert_eq!(classify_message("I can't reach that!"), Some(WorkOutcome::UnreachableSpot));
        assert_eq!(classify_message("You need a Woodcutting level of 60."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You need a Mining level of 15."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You need at least level 5 Fishing."), Some(WorkOutcome::LevelLocked));
        assert_eq!(classify_message("You do not have an axe to cut this."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You do not have a pickaxe to mine this."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You need a rod to catch these."), Some(WorkOutcome::NoTool));
        assert_eq!(classify_message("You don't have any bait left."), Some(WorkOutcome::NoBait));
        assert_eq!(classify_message("Your inventory is too full to hold any more logs."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("Your inventory is too full to hold any more iron ore."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("You can't carry any more fish."), Some(WorkOutcome::Full));
        assert_eq!(classify_message("There is no ore currently available."), Some(WorkOutcome::Gone));
        assert_eq!(classify_message("There are no minerals currently available."), Some(WorkOutcome::Gone));
        assert_eq!(classify_message("You get some oak logs."), None, "progress stays caller-side");
    }


    /// Work-start ack: per-skill vocab, case-insensitive, only lines
    /// after the click count (seen-indexed, never replays old swings).
    #[test]
    fn work_acked_detects_swing_since_click() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.messages.push("You swing your axe at the tree.".to_string());
        let seen = bot.world.messages.len();
        bot.world.messages.push("You get some yew logs.".to_string());
        assert!(!work_acked(&bot, seen, &WOODCUT_ACKS), "loot is not a start ack");
        assert!(!work_acked(&bot, seen, &MINING_ACKS), "axe swing is not a pick swing");
        bot.world.messages.push("YOU SWING YOUR PICK AT THE ROCK.".to_string());
        assert!(work_acked(&bot, seen, &MINING_ACKS), "case-insensitive pick ack");
        assert!(!work_acked(&bot, 0, &FISHING_ACKS), "no cast, no ack");
        bot.world.messages.push("You attempt to catch a fish.".to_string());
        assert!(work_acked(&bot, seen, &FISHING_ACKS), "attempt counts as fishing ack");
    }


    /// Overlay-layer modals never block: the wilderness overlay (live 193)
    /// reads as dismissed with no server round-trip, so the gather loops
    /// reach the real message ("can't reach that") instead of Blocked.
    #[test]
    fn overlay_modal_reads_as_dismissed() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.modal = Some(lifescape_sdk::world::Modal { layer: "overlay".to_string(), interface_id: 193 });
        assert!(dismiss_modal(&mut bot).expect("overlay never errors"), "overlay is non-blocking");
        assert!(bot.world.modal.is_some(), "overlay stays open, only the verdict changes");
        bot.world.modal = None;
        assert!(dismiss_modal(&mut bot).expect("no modal passes"), "no modal passes");
    }
}
