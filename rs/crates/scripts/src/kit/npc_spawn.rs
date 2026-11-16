//! A working NPC out of sight (Aggie the dyer, a tanner): find it, else
//! walk to the nearest spawn the dump places it at and look again (live
//! admin 2026-11-15: `dye` failed "no produce NPC aggie" from every bank
//! but Draynor's).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::ScriptError;

/// Slot and name of the first of `names` in sight, walking to its nearest
/// spawn first when none is (`None`: still out of sight there).
pub fn find_or_walk(bot: &mut Bot, names: &[&str], verb: &str) -> Result<Option<(u16, String)>, ScriptError> {
    let find = |bot: &Bot| names.iter().find_map(|name| bot.find_npc(name).map(|found| (found, (*name).to_string())));
    if let Some(found) = find(bot) {
        return Ok(Some(found));
    }
    let Some(spawn) = nearest_spawn(bot, names) else { return Ok(None) };
    bot.log(&format!("{verb} {names:?} out of sight: walking to its spawn {spawn:?}"));
    bot.travel_to(lifescape_sdk::plan::Tile { x: spawn.0, z: spawn.1, level: spawn.2 })?;
    Ok(find(bot))
}

/// Nearest dump spawn of any of `names` on the bot's level.
fn nearest_spawn(bot: &Bot, names: &[&str]) -> Option<(i32, i32, i32)> {
    let here = (bot.world.x, bot.world.z, bot.world.level);
    names
        .iter()
        .flat_map(|name| knowledge::npcs_by_name(name))
        .flat_map(|npc| npc.spawns.iter())
        .map(|spot| (spot.x as i32, spot.z as i32, spot.level as i32))
        .filter(|spot| spot.2 == here.2)
        .min_by_key(|spot| (spot.0 - here.0).abs() + (spot.1 - here.1).abs())
}
