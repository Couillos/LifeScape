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

/// Click the altar from every orthogonal stand until the essence burns.
/// A single `approach` stops at the first tile in range, which can be a
/// no-reach corner (`I can't reach that!` with the pack untouched); the
/// stands walk the neighbour that crafts. Reports the last game message
/// instead of crying about missing essence.
pub(crate) fn craft_at_altar(bot: &mut Bot, essence: u16, essence_name: &str, altar: &str) -> Result<u32, ScriptError> {
    let (loc_x, loc_z, level, loc) = find_loc(bot, &[altar], 150)?;
    let (width, length) = knowledge::locs_by_name(altar)
        .first()
        .map(|entry| (entry.width as i32, entry.length as i32))
        .unwrap_or((3, 3));
    let before = pack_total(bot, essence);
    let seen = bot.world.messages.len();
    let stands = stands_around(loc_x, loc_z, width, length, bot.world.x, bot.world.z);
    for (stand_x, stand_z) in &stands {
        if let Err(error) = bot.walk_to(*stand_x, *stand_z, level, 1500) {
            if walk_failed_fatal(&error) {
                return Err(error.into());
            }
            continue;
        }
        bot.op_loc(1, loc_x, loc_z, loc)?;
        wait_ticks(bot, 10)?;
        let after = pack_total(bot, essence);
        if after < before {
            return Ok(before - after);
        }
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
    Err(ScriptError::Usage(format!("{portal} did not move us from {:?} (stands {})", bot.position(), stands.len())))
}
