//! Tools: shared tool operations. The rank itself (`ToolSpec`,
//! best-first order, level gates through `usable`) plus reattach and
//! banked-restore, used by every gather skill. Bank restores go through
//! one function: withdraw the best banked tool, else the banked
//! handle+head, never a second tool in the pack.

#![forbid(unsafe_code)]

pub use crate::skill_gather::{best_banked_by_rank, reattach_tool, BanSet, ToolSpec};

use lifescape_sdk::Bot;

use crate::skill_common::{live_bank_booth, obj_id};
use crate::ScriptError;

/// Withdraw a banked tool (or its handle+head) and reattach it. Cheaper
/// than rebuying. Reopens the bank: call after a bank trip, or not at
/// all when the bank is further than a dozen tiles (booth business from
/// across the map fails instead of fanning walks at an untouchable
/// booth).
pub fn restore_from_bank(
    bot: &mut Bot,
    bank: &lifescape_gamedata::banks::Bank,
    spec: &ToolSpec,
    handle_name: &str,
    head_suffix: &str,
) -> Result<bool, ScriptError> {
    if spec.best_held(bot).is_some() {
        return Ok(true);
    }
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        bot.log(&format!("tool restore: bank at ({},{}) too far from {:?}", bank.x, bank.z, bot.position()));
        return Ok(false);
    }
    let Ok(handle) = obj_id(bot, handle_name) else {
        return Ok(false);
    };
    let heads: Vec<u16> = lifescape_gamedata::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with(head_suffix))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    if heads.is_empty() {
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
        return Ok(false);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        return Ok(false);
    }
    let Some(main_com) = (bot.tables.com_id)("bank_main:inv") else {
        bot.close_modal()?;
        return Ok(false);
    };
    let slots = bot.world.inv.get(&main_com).cloned().unwrap_or_default();
    let known = spec.known_ids(bot);
    // One tool in the pack: withdraw the best banked one, never a second.
    let rank = |name: &str| spec.rank(name);
    if let Some((slot, held)) = best_banked_by_rank(&slots, &known, rank) {
        bot.inv_button(1, held, slot, main_com)?;
        bot.pump(10)?;
        bot.close_modal()?;
        bot.pump(5)?;
        if spec.best_held(bot).is_some() {
            bot.log("ok withdrew banked tool");
            return Ok(true);
        }
        return Ok(false);
    }
    let handle_slot = slots.iter().find(|(_, (held, _))| *held == handle).map(|(slot, _)| *slot);
    let head_slot = slots.iter().find(|(_, (held, _))| heads.contains(held)).map(|(slot, _)| *slot);
    let (Some(handle_slot), Some(head_slot)) = (handle_slot, head_slot) else {
        bot.close_modal()?;
        bot.pump(5)?;
        return Ok(false);
    };
    let head = slots[&head_slot].0;
    bot.inv_button(1, handle, handle_slot, main_com)?;
    bot.pump(5)?;
    bot.inv_button(1, head, head_slot, main_com)?;
    bot.pump(10)?;
    bot.close_modal()?;
    bot.pump(5)?;
    if reattach_tool(bot, handle_name, head_suffix, spec)? {
        bot.log("ok restored tool from banked pieces");
        return Ok(true);
    }
    Ok(false)
}
