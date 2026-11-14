//! Bank janitor bricks: the tick budget spent since a pass started,
//! the carried subset of a stack list, pack-sized withdrawal slices and
//! dropping one carried stack whole. Shared by `market cleanup`.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::free_slots;
use crate::ScriptError;

/// Ticks spent since `start_tick`, pauses (`bot.pause_ticks`) excluded.
pub(crate) fn spent(start_tick: u64, start_pause: u64, bot: &Bot) -> u64 {
    bot.world.ticks.saturating_sub(start_tick).saturating_sub(bot.pause_ticks.saturating_sub(start_pause))
}

/// The ids of `ids` the pack currently carries.
pub(crate) fn packed(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    ids.iter().filter(|id| bot.inventory().get(id).copied().unwrap_or(0) > 0).copied().collect()
}

/// Pack-sized withdrawal slice of banked ids: the pack holds 28, so every
/// pass lists, sells and drops in chunks instead of stalling full.
pub(crate) fn next_chunk(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    let mut chunk = Vec::new();
    let mut free = free_slots(bot);
    for id in ids {
        if bot.bank_items().get(id).copied().unwrap_or(0) == 0 || free == 0 {
            continue;
        }
        chunk.push(*id);
        free -= 1;
    }
    chunk
}

/// Drops every carried `id`; `false` when the tick budget ran out first.
pub(crate) fn drop_all(bot: &mut Bot, id: u16, start_tick: u64, start_pause: u64, budget_max: u64) -> Result<bool, ScriptError> {
    while bot.inventory().get(&id).copied().unwrap_or(0) > 0 {
        if spent(start_tick, start_pause, bot) >= budget_max {
            return Ok(false);
        }
        // Server truth beats a stale pack cache: already gone counts
        // as dropped, never as a failed run.
        match bot.drop(id, 750) {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) if error.to_string().contains("no such item") => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}
