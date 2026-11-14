//! Bank janitor bricks: the tick budget spent since a pass started,
//! the carried subset of a stack list, pack-sized withdrawal slices and
//! dropping one carried stack whole. Shared by `market cleanup`.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use crate::kit::pack::free_slots;
use crate::ScriptError;

/// Ticks spent since `start_tick`, pauses (`bot.pause_ticks`) excluded.
pub(crate) fn spent(start_tick: u64, start_pause: u64, bot: &Bot) -> u64 {
    bot.world.ticks.saturating_sub(start_tick).saturating_sub(bot.pause_ticks.saturating_sub(start_pause))
}

/// The ids of `ids` the BACKPACK carries. Never `inventory()`: with the
/// bank open it also sums the bank and its side mirror, so every banked
/// stack read as carried (live bst29bh9 2026-11-14: the drop loop broke
/// on a banked stack it could not drop, D17 never ran past one stack).
pub(crate) fn packed(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    ids.iter().filter(|id| bot.pack_count(**id) > 0).copied().collect()
}

/// Last known bank, per object: live while the dialog is open, else the
/// contents kept at its close (`World::inv_known`). `bank_items()` reads
/// the live com only and drops to empty the moment a withdrawal closes
/// the dialog, so a closed-bank count read "cleared" and "under the
/// limit" after every stack (live bst29bh9: 60 types, one drop a pass).
pub(crate) fn known_bank(bot: &Bot) -> HashMap<u16, u32> {
    let mut bank: HashMap<u16, u32> = HashMap::new();
    let Some(com) = (bot.tables.com_id)("bank_main:inv") else { return bank };
    for (held, count) in bot.world.inv_known(com).map(|slots| slots.values().copied().collect::<Vec<_>>()).unwrap_or_default() {
        let total = bank.entry(held).or_insert(0);
        *total = total.saturating_add(count);
    }
    bank.retain(|_, count| *count > 0);
    bank
}

/// Distinct types in the last known bank.
pub(crate) fn known_bank_types(bot: &Bot) -> usize {
    known_bank(bot).len()
}

/// Pack-sized withdrawal slice of banked ids: the pack holds 28, so every
/// pass lists, sells and drops in chunks instead of stalling full.
pub(crate) fn next_chunk(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    let mut chunk = Vec::new();
    let mut free = free_slots(bot);
    for id in ids {
        if known_bank(bot).get(id).copied().unwrap_or(0) == 0 || free == 0 {
            continue;
        }
        chunk.push(*id);
        free -= 1;
    }
    chunk
}

/// Drops every carried `id`; `false` when the tick budget ran out first.
pub(crate) fn drop_all(bot: &mut Bot, id: u16, start_tick: u64, start_pause: u64, budget_max: u64) -> Result<bool, ScriptError> {
    while bot.pack_count(id) > 0 {
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
