//! Temple-side runecraft steps: altar stands and the exit portal.
//!
//! The altar answers op1 only from reach-valid stands, and the exit
//! portals are floor decals with no dump spawn and no service row, so
//! `find_loc` never sees them. Both are worked from content coordinates
//! here; `runecraft.rs` keeps the banked-loads loop. Split keeps the
//! activity brick under its line ceiling, nothing else.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;
use crate::devtools::sides_of;
use crate::kit::pack::pack_total;
use crate::kit::site::paused_since;
use crate::quests::common::find_loc;
use crate::ScriptError;

/// Exit portals by prefix (server land files `maps-server.zip`: l44_75
/// air, l43_75 mind, l42_75 water, l41_75 earth, l40_75 fire, l39_75
/// body). (prefix, loc id, x, z), level 0 throughout; ids match the dump
/// order (2452/2465/2478 seen live, the rest contiguous in one config).
pub const PORTALS: [(&str, u16, i32, i32); 6] = [
    ("air", 2465, 2841, 4828),
    ("mind", 2466, 2793, 4827),
    ("water", 2467, 2727, 4832),
    ("earth", 2468, 2655, 4829),
    ("fire", 2469, 2574, 4850),
    ("body", 2470, 2523, 4825),
];

/// Orthogonal stands around a wxl footprint cornered at (x, z), nearest
/// the bot first (same rule as the item-on-loc stands: one approach tile
/// can be a no-reach corner while its neighbour crafts fine).
fn stands_around(x: i32, z: i32, width: i32, length: i32, bot_x: i32, bot_z: i32) -> Vec<(i32, i32)> {
    let mut footprint = Vec::new();
    for dx in 0..width {
        for dz in 0..length {
            footprint.push([i64::from(x + dx), i64::from(z + dz)]);
        }
    }
    sides_of(&footprint, bot_x, bot_z)
}

/// True for a fatal socket/login error: anything else while walking to a
/// stand just tries the next stand.
fn walk_failed_fatal(error: &lifescape_sdk::Error) -> bool {
    matches!(error, lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))
}

/// Chebyshev reach of a temple around its exit portal: the altar rooms
/// span ~15 tiles from their portal, the next temple sits 50+ away.
const TEMPLE_RADIUS: i32 = 40;

/// Pure: `here` (level, x, z) stands inside the `prefix` temple (near its
/// exit portal, level 0). A market pause can take the bot out of the
/// temple to the Varrock square (live 2026-11-21 kai_ellis 05:57).
pub(crate) fn in_temple(prefix: &str, here: (i32, i32, i32)) -> bool {
    PORTALS.iter().find(|(name, _, _, _)| *name == prefix).is_some_and(|(_, _, portal_x, portal_z)| {
        here.0 == 0 && (here.1 - portal_x).abs().max((here.2 - portal_z).abs()) <= TEMPLE_RADIUS
    })
}

/// Outcome of one altar click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AltarStep {
    /// The altar bound this much essence.
    Bound(u32),
    /// A market pause closed meanwhile: the delivery banked the pack but
    /// the tools and left the bot at the Varrock square, a lower count
    /// is no craft. The load is cut, the cycle takes it again.
    PauseCut,
    /// Nothing bound from this stand (no-reach corner): next stand.
    NextStand,
}

/// Pure: read one click from the essence count `before`/`after` and
/// whether a market pause closed since the load began (read first).
pub(crate) fn altar_step(before: u32, after: u32, paused: bool) -> AltarStep {
    if paused {
        AltarStep::PauseCut
    } else if after < before {
        AltarStep::Bound(before - after)
    } else {
        AltarStep::NextStand
    }
}

/// `Failed` for a load a market pause cut (`kit::site::pause_can_explain`:
/// the cycle takes the load again, never a contract error).
fn pause_cut(altar: &str, essence_name: &str, bot: &Bot) -> ScriptError {
    ScriptError::Failed(format!("{altar}: market pause cut the {essence_name} load, now at {:?}", bot.position()))
}

/// Click the altar from every orthogonal stand until the essence burns.
/// A single `approach` stops at the first tile in range, which can be a
/// no-reach corner (`I can't reach that!` with the pack untouched); the
/// stands walk the neighbour that crafts. An empty pack or a market pause
/// during the load (the delivery banks the pack, live 2026-11-21) fails
/// typed `Failed`: the world's state, retaken by the cycle. `Usage` only
/// when the essence stayed in the pack through every stand, no pause.
pub(crate) fn craft_at_altar(bot: &mut Bot, essence: u16, essence_name: &str, altar: &str) -> Result<u32, ScriptError> {
    let before = pack_total(bot, essence);
    if before == 0 {
        return Err(ScriptError::Failed(format!("{altar}: no {essence_name} in the pack at {:?}", bot.position())));
    }
    let pauses_mark = bot.pauses_done;
    let (loc_x, loc_z, level, loc) = find_loc(bot, &[altar], 150)?;
    let (width, length) = knowledge::locs_by_name(altar)
        .first()
        .map(|entry| (entry.width as i32, entry.length as i32))
        .unwrap_or((3, 3));
    let seen = bot.world.messages.len();
    let stands = stands_around(loc_x, loc_z, width, length, bot.world.x, bot.world.z);
    for (stand_x, stand_z) in &stands {
        if let Err(error) = bot.walk_to(*stand_x, *stand_z, level, 1500) {
            if walk_failed_fatal(&error) {
                return Err(error.into());
            }
            if paused_since(pauses_mark, bot.pauses_done) {
                return Err(pause_cut(altar, essence_name, bot));
            }
            continue;
        }
        bot.op_loc(1, loc_x, loc_z, loc)?;
        wait_ticks(bot, 10)?;
        match altar_step(before, pack_total(bot, essence), paused_since(pauses_mark, bot.pauses_done)) {
            AltarStep::Bound(bound) => return Ok(bound),
            AltarStep::PauseCut => return Err(pause_cut(altar, essence_name, bot)),
            AltarStep::NextStand => {}
        }
    }
    if paused_since(pauses_mark, bot.pauses_done) {
        return Err(pause_cut(altar, essence_name, bot));
    }
    let last = bot.world.messages.get(seen..).unwrap_or_default().last().cloned().unwrap_or_default();
    Err(ScriptError::Usage(format!(
        "{altar} ate no {essence_name} from {} stands at {:?} (last game msg: {last:?})",
        stands.len(),
        bot.position()
    )))
}

/// Step out through the exit portal: walk its stands, click op1, stop when
/// the telejump moves us. Never silently strands: failure is a loud Usage.
pub(crate) fn exit_temple(bot: &mut Bot, prefix: &str, portal: &str) -> Result<(), ScriptError> {
    let Some((_, portal_id, portal_x, portal_z)) = PORTALS.iter().find(|(name, _, _, _)| *name == prefix).copied() else {
        return Err(ScriptError::Usage(format!("no exit portal row for {prefix:?}")));
    };
    let _ = portal;
    let stands = stands_around(portal_x, portal_z, 1, 1, bot.world.x, bot.world.z);
    for (stand_x, stand_z) in &stands {
        // A market pause took the bot out of the temple (the delivery
        // left it at the Varrock square): the portal is behind us, a walk
        // to its stands is a NoRoute dossier per stand (live 2026-11-22,
        // erin_quill: 4 dossiers in 12 s from (3185,3436) to the earth
        // altar). Outside the temple the exit already happened.
        if !in_temple(prefix, bot.position()) {
            bot.log(&format!("exit {prefix} temple: already outside at {:?}, nothing to exit", bot.position()));
            return Ok(());
        }
        if let Err(error) = bot.walk_to(*stand_x, *stand_z, bot.world.level, 1500) {
            if walk_failed_fatal(&error) {
                return Err(error.into());
            }
            continue;
        }
        let (before_level, before_x, before_z) = bot.position();
        bot.op_loc(1, portal_x, portal_z, portal_id)?;
        wait_ticks(bot, 10)?;
        let (after_level, after_x, after_z) = bot.position();
        if after_level != before_level || (after_x - before_x).abs().max((after_z - before_z).abs()) > 2 {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{portal} did not move us from {:?} (stands {})", bot.position(), stands.len())))
}

#[cfg(test)]
mod tests {
    use super::{altar_step, craft_at_altar, in_temple, AltarStep, PORTALS};
    use crate::kit::pack::obj_id;
    use crate::kit::site::pause_can_explain;
    use crate::tables_live::attach_live_tables;
    use crate::ScriptError;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Live 2026-11-21 kai_ellis 07:08: a market delivery banked the 26
    /// essence mid-trip (`bank_deposit_all ... Rune essence`), the trip
    /// replanned and the bot clicked the earth altar from 12 stands with
    /// an empty pack: `Usage` banned runecraft and magic. An empty pack
    /// is the world's state, never a contract error: typed `Failed`,
    /// which the cycle retakes after a pause (`pause_can_explain`).
    #[test]
    fn empty_pack_at_the_altar_is_no_contract_error() {
        let mut bot = offline_bot();
        let essence = obj_id(&bot, "blankrune").expect("essence id");
        let outcome = craft_at_altar(&mut bot, essence, "blankrune", "earth_altar");
        match outcome {
            Err(error @ ScriptError::Failed(_)) => assert!(pause_can_explain(&error), "the cycle retakes it after a pause"),
            other => panic!("empty pack must fail typed Failed, got {other:?}"),
        }
    }

    /// One altar click: a closed pause cuts the load whatever the pack
    /// shows (the delivery banked it: a lower count is no craft), a lower
    /// count without pause is the bound essence, else the next stand.
    #[test]
    fn altar_step_reads_pause_before_the_pack() {
        assert_eq!(altar_step(26, 0, true), AltarStep::PauseCut, "banked by the delivery, not bound");
        assert_eq!(altar_step(26, 26, true), AltarStep::PauseCut, "pause, nothing bound");
        assert_eq!(altar_step(26, 0, false), AltarStep::Bound(26), "all bound");
        assert_eq!(altar_step(26, 26, false), AltarStep::NextStand, "no reach from this stand");
    }

    /// Live 2026-11-22 erin_quill 06:51: the pause left the bot at the
    /// Varrock square before the exit portal was worked; the exit walked
    /// to the portal stands inside the altar room (4 NoRoute dossiers).
    /// Outside the temple there is nothing to exit: Ok, no walk.
    #[test]
    fn exit_from_outside_the_temple_is_a_no_op() {
        let mut bot = offline_bot();
        (bot.world.level, bot.world.x, bot.world.z) = (0, 3185, 3436);
        super::exit_temple(&mut bot, "earth", "earthtemple_exit_portal").expect("already outside: nothing to do");
    }

    /// Live 2026-11-21 kai_ellis 05:57: the pause took the bot from the
    /// air temple to the Varrock square; the exit portal is only tried
    /// from inside the temple (no 8 nav dossiers from Varrock).
    #[test]
    fn temple_test_reads_the_portal_area() {
        for (prefix, _, x, z) in PORTALS {
            assert!(in_temple(prefix, (0, x + 2, z + 10)), "{prefix} altar room");
        }
        assert!(!in_temple("air", (0, 3185, 3436)), "Varrock square");
        assert!(!in_temple("air", (1, 2841, 4828)), "other level");
        assert!(!in_temple("nature", (0, 2841, 4828)), "no portal row");
    }
}
