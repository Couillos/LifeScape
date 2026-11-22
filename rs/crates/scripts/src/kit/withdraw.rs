//! Withdraw: pull named items out of the bank, walking to it first when
//! the last known bank view (open, or closed and kept in `inv_closed`,
//! seeded from the save at login) holds a usable unit. A clean pack
//! makes `bank_trip` a no-op, so without this walk a tool or an input
//! resting in a bank 20 tiles away read as missing (live 2026-11-13:
//! will_field blocked "missing tinderbox" with two banked, "bank at
//! (3252,3419) too far from (3240,3427)").

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::live_bank_booth;
use crate::kit::pack::{banked_usable, pack_total};
use crate::ScriptError;

/// Manhattan reach of a booth from where the bot stands: further than
/// this, booth business needs a walk first.
pub const BOOTH_REACH: i32 = 12;

/// Walk budget to fetch a banked item from across the map (one bank
/// trip transit slice x 2).
pub const BANK_FETCH_WALK_TICKS: u64 = 3000;

/// Inputs `(name, per action)` that pack + known bank cannot cover for
/// one action (plan P17, generalising c2a4065: check at the start, never
/// after the trip; live 2026-11-17: 150 smelt runs walked ~955 ticks to
/// block on coal the bank never had). Empty while the bank was never
/// seen (`bank_known` false): the trip itself finds out.
pub fn preflight_short(inputs: &[(String, i64)], held: &dyn Fn(&str) -> i64, bank_known: bool) -> Vec<String> {
    if !bank_known {
        return Vec::new();
    }
    inputs.iter().filter(|(name, each)| held(name) < (*each).max(1)).map(|(name, _)| name.clone()).collect()
}

/// `preflight_short` on the live bot: pack plus the last known bank.
pub fn preflight_blocked(bot: &Bot, inputs: &[(String, i64)]) -> Option<ScriptError> {
    let bank_known = (bot.tables.com_id)("bank_main:inv").is_some_and(|com| bot.world.inv_known(com).is_some());
    // The banknotes of an input in the pack count: the bank trip that
    // follows deposits them (live 2026-11-22, a shop trip's unsold notes
    // read as `missing: iron_ore`).
    let held = |name: &str| -> i64 {
        crate::kit::pack::obj_id(bot, name)
            .map(|obj| {
                let note = crate::market_client::trade_id(obj);
                let noted = if note == obj { 0 } else { pack_total(bot, note) };
                i64::from(pack_total(bot, obj)) + i64::from(noted) + i64::from(banked_usable(bot, obj))
            })
            .unwrap_or(0)
    };
    let missing = preflight_short(inputs, &held, bank_known);
    (!missing.is_empty()).then_some(ScriptError::Blocked { missing })
}

/// True when the bank is out of booth reach from `(x, z)`.
pub fn bank_far(bank: &knowledge::banks::Bank, x: i32, z: i32) -> bool {
    (x - bank.x as i32).abs() + (z - bank.z as i32).abs() > BOOTH_REACH
}

/// Pure verdict: walk to the bank before withdrawing, because it is out
/// of reach and the known bank view holds a usable unit of one of `ids`.
/// Nothing known banked never walks (no empty treks across the map).
pub fn fetch_walk_needed(bot: &Bot, bank: &knowledge::banks::Bank, ids: &[u16]) -> bool {
    bank_far(bank, bot.world.x, bot.world.z) && ids.iter().any(|id| banked_usable(bot, *id) > 0)
}

/// Walk to the bank for known banked items (see `fetch_walk_needed`).
/// Connection, login and operator stops abort typed; a failed walk only
/// logs (the reach check right after reports it).
pub fn walk_to_bank_for(bot: &mut Bot, bank: &knowledge::banks::Bank, what: &str) -> Result<(), ScriptError> {
    bot.log(&format!("walking to the bank at ({},{}) for banked {what}", bank.x, bank.z));
    bot.intend(&format!("walking to the bank at ({},{}) for {what}", bank.x, bank.z));
    match bot.approach(bank.x as i32, bank.z as i32, bank.level as i32, 6, BANK_FETCH_WALK_TICKS) {
        Ok(()) => Ok(()),
        Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => Err(ScriptError::Sdk(error)),
        Err(other) => {
            bot.log(&format!("walk to the bank for {what} failed: {other}"));
            Ok(())
        }
    }
}

/// Reach the bank for a refill: walks when it is out of reach and the
/// known view holds one of `ids`. True when the booth is in reach after
/// (fill helpers withdraw), false when far with nothing known banked
/// (the caller reports its `blocked`, never an empty trek).
pub fn reach_bank_for(bot: &mut Bot, bank: &knowledge::banks::Bank, ids: &[u16]) -> Result<bool, ScriptError> {
    if fetch_walk_needed(bot, bank, ids) {
        let label = ids.iter().map(|id| (bot.tables.obj_name)(*id).unwrap_or_else(|| id.to_string())).collect::<Vec<_>>().join(", ");
        walk_to_bank_for(bot, bank, &label)?;
    }
    Ok(!bank_far(bank, bot.world.x, bot.world.z))
}

/// Reach the bank (walking when far with one of `ids` known banked) and
/// open a live booth. False, logged, when any step falls short: the
/// caller withdraws nothing and reports from what the pack holds.
pub fn open_bank_for(bot: &mut Bot, bank: &knowledge::banks::Bank, ids: &[u16]) -> Result<bool, ScriptError> {
    if !reach_bank_for(bot, bank, ids)? {
        bot.log(&format!("bank withdraw: bank at ({},{}) too far from {:?}", bank.x, bank.z, bot.position()));
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
        bot.log(&format!("bank withdraw: no live bankbooth near ({},{})", bank.x, bank.z));
        return Ok(false);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        bot.log(&format!("bank withdraw: bank did not open at {:?}", bot.position()));
        return Ok(false);
    }
    Ok(true)
}

/// Withdraw exact counts of several items in one booth visit (spell loads:
/// runes and targets). `bot.bank_withdraw` presses one 1/5/10/All/X, honours
/// market locks and ends quietly on a full pack. A banknote id (`cert_`)
/// comes out of its base stack in note mode (`bank_withdraw_noted`).
/// Connection and operator stops abort typed; any other refusal is
/// logged and the caller judges the pack. Returns the units that came out.
pub fn bank_withdraw_counts(bot: &mut Bot, bank: &knowledge::banks::Bank, wants: &[(u16, u32)]) -> Result<u32, ScriptError> {
    let wants: Vec<(u16, u32)> = wants.iter().copied().filter(|(_, count)| *count > 0).collect();
    let ids: Vec<u16> = wants.iter().map(|(id, _)| *id).collect();
    // The bank holds base objects only: the walk/open check reads those.
    let banked_ids: Vec<u16> = ids.iter().map(|id| crate::market_lots::note_base(*id).unwrap_or(*id)).collect();
    if ids.is_empty() || !open_bank_for(bot, bank, &banked_ids)? {
        return Ok(0);
    }
    let before: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    for (id, count) in wants {
        let pulled = match crate::market_lots::note_base(id) {
            Some(base) => bot.bank_withdraw_noted(base, id, count, 50),
            None => bot.bank_withdraw(id, count, 50),
        };
        match pulled {
            Ok(_) => {}
            Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => return Err(ScriptError::Sdk(error)),
            Err(other) => bot.log(&format!("bank withdraw of {} x{count}: {other}", crate::kit::pack::obj_label(bot, id))),
        }
    }
    bot.close_modal()?;
    bot.pump(5)?;
    let after: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    Ok(after.saturating_sub(before))
}

/// Withdraw up to `count` items total, across `ids`, from the bank at
/// `bank`. Returns how many came out of the bank. Walks to the bank first
/// when it is out of reach and the known view holds one of `ids`; with
/// nothing known banked, a far bank fails fast (0). Bounded in ticks.
pub fn bank_withdraw_items(
    bot: &mut Bot,
    bank: &knowledge::banks::Bank,
    ids: &[u16],
    count: u32,
) -> Result<u32, ScriptError> {
    if count == 0 || ids.is_empty() {
        return Ok(0);
    }
    if !open_bank_for(bot, bank, ids)? {
        return Ok(0);
    }
    let Some(main_com) = (bot.tables.com_id)("bank_main:inv") else {
        bot.close_modal()?;
        return Ok(0);
    };
    let before: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    let mut remaining = count;
    // One SDK transfer per stack (`bot.bank_withdraw`: 1/5/10/All/X, one
    // press, market locks honoured); the old 5/1 presses with a blind
    // 5-pump wait spammed 25 presses for 25 runes.
    let mut tried: Vec<u16> = Vec::new();
    while remaining > 0 {
        let slots = bot.world.inv.get(&main_com).cloned().unwrap_or_default();
        let mut stacks: Vec<(u8, u16, u32)> = slots
            .iter()
            .filter(|(_, (held, n))| ids.contains(held) && *n > 0 && !tried.contains(held))
            .map(|(slot, (held, n))| (*slot, *held, bot.withdrawable(*held, *n)))
            .filter(|(_, _, usable)| *usable > 0)
            .collect();
        stacks.sort_unstable();
        let Some((_, held, usable)) = stacks.first().copied() else { break };
        tried.push(held);
        let had = pack_total(bot, held);
        match bot.bank_withdraw(held, remaining.min(usable), 50) {
            Ok(_) => {}
            Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => return Err(ScriptError::Sdk(error)),
            Err(other) => bot.log(&format!("bank withdraw of {} x{}: {other}", crate::kit::pack::obj_label(bot, held), remaining.min(usable))),
        }
        remaining = remaining.saturating_sub(pack_total(bot, held).saturating_sub(had));
    }
    bot.close_modal()?;
    bot.pump(5)?;
    let after: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    Ok(after.saturating_sub(before))
}

/// Make sure one unit of a tool sits in the pack: pack first, then the
/// bank (walking to it when the known view holds one). `Blocked` names
/// the tool when neither has it, so the brain files the obtain.
pub fn ensure_tool(bot: &mut Bot, bank: &knowledge::banks::Bank, tool: u16, tool_name: &str) -> Result<(), ScriptError> {
    if pack_total(bot, tool) > 0 {
        return Ok(());
    }
    if bank_withdraw_items(bot, bank, &[tool], 1)? > 0 {
        bot.log(&format!("ok {tool_name} restored from bank"));
    }
    if pack_total(bot, tool) == 0 {
        return Err(ScriptError::Blocked { missing: vec![tool_name.to_string()] });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Plan P17: a smelt cycle without coal blocks before any walk when
    /// the bank is known; an unseen bank never concludes.
    #[test]
    fn inputs_are_checked_before_the_trip() {
        let steel = vec![("iron_ore".to_string(), 1), ("coal".to_string(), 2)];
        let no_coal = |name: &str| if name == "iron_ore" { 10 } else { 0 };
        assert_eq!(super::preflight_short(&steel, &no_coal, true), vec!["coal".to_string()]);
        assert!(super::preflight_short(&steel, &no_coal, false).is_empty(), "bank unseen: the trip finds out");
        let stocked = |name: &str| if name == "iron_ore" { 10 } else { 20 };
        assert!(super::preflight_short(&steel, &stocked, true).is_empty());
    }

    /// Live 2026-11-22: the pack holds 7 637 iron ore as banknotes (a
    /// shop trip's unsold lot), the bank none: not missing, the bank
    /// trip deposits them. Nothing anywhere stays missing.
    #[test]
    fn banknotes_in_the_pack_are_not_missing() {
        let mut bot = bank_view_bot(3268, 3170, &[("coal", 40)]);
        let pack = (bot.tables.com_id)("inventory:inv").expect("pack com");
        let ore = obj_id(&bot, "iron_ore").expect("iron ore");
        let note = crate::market_client::trade_id(ore);
        assert_ne!(note, ore, "iron ore has a banknote");
        let inputs = vec![("iron_ore".to_string(), 1i64), ("coal".to_string(), 2i64)];
        assert!(super::preflight_blocked(&bot, &inputs).is_some(), "no ore anywhere: blocked");
        bot.world.inv.insert(pack, [(0u8, (note, 7637u32))].into_iter().collect());
        assert!(super::preflight_blocked(&bot, &inputs).is_none(), "the notes count");
    }

    use super::{bank_far, fetch_walk_needed};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn bank_view_bot(x: i32, z: i32, banked: &[(&str, u32)]) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.level = 0;
        bot.world.x = x;
        bot.world.z = z;
        let com = (bot.tables.com_id)("bank_main:inv").expect("bank_main:inv in dumps");
        let counts = banked.iter().map(|(name, count)| (obj_id(&bot, name).expect("obj resolves"), *count)).collect();
        bot.world.seed_closed_inv(com, &counts);
        bot
    }

    /// will_field (live 2026-11-13): clean pack at (3240,3427), two
    /// tinderboxes in the bank, Varrock east booth 20 tiles away. The
    /// fetch walks instead of reporting the tinderbox missing; nothing
    /// banked never walks; a bank in reach needs no walk.
    #[test]
    fn far_bank_with_banked_tool_walks_first() {
        let bank = crate::kit::spots::nearest_bank_at(3252, 3419, 0).expect("Varrock east bank");
        assert!(bank_far(&bank, 3240, 3427), "20 tiles is out of booth reach");
        let bot = bank_view_bot(3240, 3427, &[("tinderbox", 2)]);
        let tinderbox = obj_id(&bot, "tinderbox").expect("tinderbox");
        assert!(fetch_walk_needed(&bot, &bank, &[tinderbox]), "banked tinderbox: walk to the bank");
        let hammer = obj_id(&bot, "hammer").expect("hammer");
        assert!(!fetch_walk_needed(&bot, &bank, &[hammer]), "nothing known banked: no empty trek");
        let near = bank_view_bot(bank.x as i32, bank.z as i32 + 1, &[("tinderbox", 2)]);
        assert!(!fetch_walk_needed(&near, &bank, &[tinderbox]), "booth in reach: no walk");
    }
}
