//! Tool upgrade at the bank (live 2026-11-16, jim_ellis: woodcutting 83
//! cutting yews with a bronze axe in the pack while an iron axe rested in
//! its bank; `reclaim_free` returned as soon as any axe was held). Each
//! bank visit, and the run start, swaps the held tool for the best banked
//! one of its family the level works with: withdraw the better, deposit
//! the lesser pack copies, never two of a family in the pack. A worn tool
//! stays worn (the gear plan owns the right hand); the better one rides
//! in the pack, where the server works with it (`woodcutting_axe_checker`,
//! `pickaxe_checker.rs2`: best of worn and pack).
//!
//! Families: axes and pickaxes only. Fishing tools are per-tier, never
//! ranks of one family (a harpoon is no better net): the fishing gear
//! pass withdraws the exact tool of its tier (`fish_gear::ensure_gear`).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{banked_usable, obj_id, pack_ids, pack_total};
use crate::kit::tools::{ToolSpec, AXE_SPEC, PICK_SPEC};
use crate::ScriptError;

/// Families a bank visit upgrades: spec, gear family
/// (`gamedata::gear::usable_tools`), skill stat.
pub const UPGRADE_FAMILIES: [(&ToolSpec, &str, u8); 2] =
    [(&AXE_SPEC, "axe", crate::kit::woodcut::WC_STAT), (&PICK_SPEC, "pickaxe", crate::kit::mine::MINING_STAT)];

/// The banked tool a visit swaps in: the best-ranked banked name the
/// level works with (`usable`), strictly better than the best usable
/// held one. `None`: keep what is held. Pure.
pub fn upgrade_pick(spec: &ToolSpec, held: &[String], banked: &[String], usable: &[String]) -> Option<String> {
    let works = |name: &String| usable.contains(name);
    let held_rank = held.iter().filter(|name| works(name)).filter_map(|name| spec.rank(name)).min().unwrap_or(usize::MAX);
    banked
        .iter()
        .filter(|name| works(name))
        .filter_map(|name| spec.rank(name).map(|rank| (rank, name)))
        .filter(|(rank, _)| *rank < held_rank)
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, name)| name.clone())
}

/// Names of `spec` held now: pack and worn.
fn held_names(bot: &Bot, spec: &ToolSpec) -> Vec<String> {
    let mut held = pack_ids(bot);
    if let Some(worn_com) = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME) {
        if let Some(slots) = bot.world.inv.get(&worn_com) {
            held.extend(slots.values().map(|(id, _)| *id));
        }
    }
    spec.order.iter().filter(|name| obj_id(bot, name).is_ok_and(|id| held.contains(&id))).map(|name| name.to_string()).collect()
}

/// Live choice: the banked upgrade of `spec` for this bot (pack and worn
/// held, last known bank view, current skill level). `None` without a
/// held tool of the family (that is the restore's job).
pub fn banked_upgrade(bot: &Bot, spec: &ToolSpec, family: &str, stat: u8) -> Option<String> {
    let held = held_names(bot, spec);
    if held.is_empty() {
        return None;
    }
    let banked: Vec<String> = spec
        .order
        .iter()
        .filter(|name| obj_id(bot, name).is_ok_and(|id| banked_usable(bot, id) > 0))
        .map(|name| name.to_string())
        .collect();
    let skill = if family == "axe" { "woodcutting" } else { "mining" };
    let level = crate::kit::survival::learn_level(bot, stat, 1);
    let usable = knowledge::gear::usable_tools(family, &[(skill, level)]);
    upgrade_pick(spec, &held, &banked, &usable)
}

/// Swap in the banked upgrade of one family, bank OPEN: withdraw the
/// better tool, then deposit every other pack copy of the family (one
/// tool in the pack, a worn one untouched). True when swapped.
pub fn swap_open(bot: &mut Bot, spec: &ToolSpec, family: &str, stat: u8) -> Result<bool, ScriptError> {
    let Some(better) = banked_upgrade(bot, spec, family, stat) else {
        return Ok(false);
    };
    let held = held_names(bot, spec);
    let better_id = obj_id(bot, &better)?;
    bot.bank_withdraw(better_id, 1, 50)?;
    if pack_total(bot, better_id) == 0 {
        bot.log(&format!("tool upgrade: {better} did not leave the bank (held {held:?}, {} free slots)", crate::kit::pack::free_slots(bot)));
        return Ok(false);
    }
    let lesser: Vec<u16> = spec.known_ids(bot).into_iter().filter(|id| *id != better_id && pack_total(bot, *id) > 0).collect();
    for id in lesser {
        bot.bank_deposit(id, u32::MAX, 50)?;
    }
    bot.pump(2)?;
    bot.log(&format!("ok tool upgrade: withdrew banked {better}, banked the lesser (held {held:?})"));
    Ok(true)
}

/// Bank visit hook (`bank::bank_trip`, bank OPEN): every upgrade family
/// the trip keeps a tool of (`keep`) swaps in its banked upgrade.
pub fn upgrade_kept(bot: &mut Bot, keep: &[u16]) -> Result<(), ScriptError> {
    for (spec, family, stat) in UPGRADE_FAMILIES {
        if spec.known_ids(bot).iter().any(|id| keep.contains(id)) && !held_names(bot, spec).is_empty() {
            swap_open(bot, spec, family, stat)?;
        }
    }
    Ok(())
}

/// Run start (`tools::reclaim_free` with a tool held): when the known
/// bank view holds an upgrade of `spec`, walk to the run's bank if far,
/// open it, swap, close. True when swapped; nothing known, nothing done.
pub fn upgrade_at_bank(bot: &mut Bot, bank: &knowledge::banks::Bank, spec: &ToolSpec) -> Result<bool, ScriptError> {
    let Some((_, family, stat)) = UPGRADE_FAMILIES.iter().find(|(family_spec, _, _)| family_spec.order == spec.order).copied() else {
        return Ok(false);
    };
    let Some(better) = banked_upgrade(bot, spec, family, stat) else {
        return Ok(false);
    };
    let better_id = obj_id(bot, &better)?;
    bot.log(&format!("tool upgrade: banked {better} beats the held one, fetching it"));
    if !crate::kit::withdraw::open_bank_for(bot, bank, &[better_id])? {
        return Ok(false);
    }
    let swapped = swap_open(bot, spec, family, stat)?;
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(swapped)
}

#[cfg(test)]
#[path = "tool_upgrade_tests.rs"]
mod tests;
