//! Shared quest solver kit: port of `scripts/quests/solver.py` (+ the bank
//! half of `scripts/quests/common.py`). Every wait is a pump loop to an
//! observable state plus a tick bound, never wall-clock sleep.

use lifescape_gamedata::{self as gamedata, quest_requirements::QuestRequirements};
use lifescape_sdk::{Bot, Error};
use lifescape_sdk::waits::wait_ticks;

use crate::{ScriptError, ScriptResult};

/// Object id for a debugname, failing loudly on unknown names.
pub fn obj_id(name: &str) -> Result<u16, ScriptError> {
    gamedata::objs_by_name(name)
        .into_iter()
        .next()
        .map(|entry| entry.obj_id as u16)
        .ok_or_else(|| ScriptError::Sdk(Error::State(format!("unknown object {name:?}"))))
}

/// Varp id for a debugname, failing loudly on unknown names.
pub fn varp_id(name: &str) -> Result<u16, ScriptError> {
    gamedata::varps::id_by_name(name)
        .and_then(|id| u16::try_from(id).ok())
        .ok_or_else(|| ScriptError::Sdk(Error::State(format!("unknown varp {name:?}"))))
}

/// Live varp value, defaulting to 0 when the server never sent it.
pub fn varp(bot: &Bot, name: &str) -> i32 {
    varp_id(name).ok().and_then(|id| bot.world.varps.get(&id).copied()).unwrap_or(0)
}

/// True when any chat text or game message contains `needle`.
pub fn has_message(bot: &Bot, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    bot.world.texts.values().any(|line| line.to_lowercase().contains(&needle))
        || bot.world.messages.iter().any(|line| line.to_lowercase().contains(&needle))
}

/// (backpack com, slot) holding `obj`, or a state error when absent.
pub fn inv_slot(bot: &Bot, obj: u16) -> Result<(u16, u8), ScriptError> {
    let (com, slots) = bot.backpack();
    let com = com.ok_or_else(|| ScriptError::Sdk(Error::State("backpack not streamed".to_string())))?;
    slots.into_iter().find(|(_, (held, _))| *held == obj).map(|(slot, _)| (com, slot)).ok_or_else(|| {
        ScriptError::Sdk(Error::State(format!("item {obj} is not in the backpack at {:?}", bot.position())))
    })
}

/// Count of `obj` carried in the backpack.
pub fn count(bot: &Bot, obj: u16) -> u32 {
    bot.inventory().get(&obj).copied().unwrap_or(0)
}

/// Count of `obj` worn (wornitems:wear container).
pub fn worn_count(bot: &Bot, obj: u16) -> u32 {
    let Some(worn_com) = gamedata::com_id("wornitems:wear") else { return 0 };
    bot.world.inv.get(&worn_com).map(|slots| slots.values().filter(|(held, _)| *held == obj).map(|(_, n)| *n).sum()).unwrap_or(0)
}

/// Nearest loc whose debugname contains one of `names`, on this level and
/// within `radius` tiles. Pumps briefly first: freshly streamed areas take
/// a few ticks to arrive.
pub fn find_loc(bot: &mut Bot, names: &[&str], radius: i32) -> Result<(i32, i32, i32, u16), ScriptError> {
    let wants: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    bot.ensure_area(here_x, here_z, radius);
    let target = bot.world.ticks + 20;
    loop {
        let mut best: Option<(i32, i32, i32, i32, u16)> = None;
        for (tile, loc) in bot.world.static_locs.iter().map(|(tile, loc)| (*tile, Some(*loc))).chain(bot.world.locs.iter().map(|(tile, loc)| (*tile, *loc))) {
            let Some((loc_id, _, _)) = loc else { continue };
            if tile.2 != bot.world.level {
                continue;
            }
            let dist = (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs();
            if dist > radius {
                continue;
            }
            let label = gamedata::loc_by_id(loc_id as usize).and_then(|entry| entry.name).unwrap_or_default().to_lowercase();
            if !wants.iter().any(|want| label.contains(want)) {
                continue;
            }
            if best.is_none_or(|known| dist < known.0) {
                best = Some((dist, tile.0, tile.1, tile.2, loc_id));
            }
        }
        if let Some((_, x, z, level, loc)) = best {
            return Ok((x, z, level, loc));
        }
        if bot.world.ticks >= target {
            return Err(ScriptError::Sdk(Error::Unreachable(format!("no loc {names:?} near {:?} (radius {radius})", bot.position()))));
        }
        bot.pump(2)?;
    }
}

/// Walk to an NPC: an explicit spot when given, else its nearest F2P spawn.
pub fn go_to_npc(bot: &mut Bot, name: &str, near: Option<(i32, i32)>) -> Result<(), ScriptError> {
    if let Some((near_x, near_z)) = near {
        if (near_x - bot.world.x).abs() + (near_z - bot.world.z).abs() > 2 {
            bot.intend(&format!("walking to {name} near {near_x},{near_z}"));
            bot.approach(near_x, near_z, bot.world.level, 2, 15000)?;
        }
        bot.intend(&format!("talking to {name}"));
        return Ok(());
    }
    let mut spawns: Vec<(i32, i32, i32)> = gamedata::npcs_by_name(name)
        .into_iter()
        .filter(|entry| entry.members != Some(true))
        .flat_map(|entry| entry.spawns)
        .map(|spot| (spot.x as i32, spot.z as i32, spot.level as i32))
        .collect();
    if spawns.is_empty() {
        return Err(ScriptError::Sdk(Error::State(format!("unknown NPC {name:?} (check the debugname in gamedata)"))));
    }
    spawns.sort_by_key(|spot| (spot.0 - bot.world.x).abs() + (spot.1 - bot.world.z).abs());
    let (spot_x, spot_z, spot_level) = spawns[0];
    if spot_level != bot.world.level || (spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs() > 2 {
        bot.intend(&format!("walking to {name} at {spot_x},{spot_z},{spot_level}"));
        bot.approach(spot_x, spot_z, spot_level, 2, 15000)?;
    }
    bot.intend(&format!("talking to {name}"));
    Ok(())
}

/// Talk to an NPC from the best adjacent tile, with one relogin retry when
/// the NPC stream flickers. True when a dialogue opened.
pub fn talk(bot: &mut Bot, name: &str, near: Option<(i32, i32)>, timeout: u64) -> Result<bool, ScriptError> {
    if scan_and_talk(bot, name, near, timeout)? {
        return Ok(true);
    }
    bot.intend("relogging to resync NPCs");
    if bot.relogin().is_ok() {
        let _ = bot.close_modal();
        bot.pump(5)?;
        if scan_and_talk(bot, name, None, timeout)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scan_and_talk(bot: &mut Bot, name: &str, near: Option<(i32, i32)>, timeout: u64) -> Result<bool, ScriptError> {
    if let Some((near_x, near_z)) = near {
        if (bot.world.x - near_x).abs() + (bot.world.z - near_z).abs() > 4 {
            let _ = bot.walk_to(near_x, near_z, bot.world.level, 1250);
        }
    }
    let target = bot.world.ticks + timeout;
    let mut slot = None;
    while bot.world.ticks < target && slot.is_none() {
        bot.pump(5)?;
        slot = bot.find_npc(name);
    }
    let Some(slot) = slot else { return Ok(false) };
    // Click from where we stand first, then from adjacent tiles.
    let npc_pos = bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z));
    if click_npc(bot, slot)? {
        return Ok(true);
    }
    if let Some((npc_x, npc_z)) = npc_pos {
        let mut tiles: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (npc_x + dx, npc_z + dz))).filter(|tile| *tile != (npc_x, npc_z)).collect();
        tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if bot.walk_to(tile_x, tile_z, bot.world.level, 750).is_ok() && click_npc(bot, slot)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn click_npc(bot: &mut Bot, slot: u16) -> Result<bool, ScriptError> {
    let seen = bot.world.messages.len();
    bot.talk_to(slot)?;
    let target = bot.world.ticks + 15;
    while bot.world.modal.is_none() && bot.world.ticks < target {
        bot.pump(5)?;
        if bot.world.messages[seen..].iter().any(|line| line.to_lowercase().contains("can't reach that")) {
            break;
        }
    }
    Ok(bot.world.modal.is_some())
}

/// Talk, answering with the first keyword-matching option (first option as
/// fallback so a choice page never stalls forever).
pub fn talk_like(bot: &mut Bot, name: &str, keywords: &[&str], near: Option<(i32, i32)>, timeout: u64) -> Result<(), ScriptError> {
    go_to_npc(bot, name, near)?;
    if bot.world.modal.is_some() {
        bot.close_modal()?;
        bot.pump(5)?;
    }
    if !talk(bot, name, near, timeout)? {
        return Err(ScriptError::Sdk(Error::Unreachable(format!("cannot talk to {name} at {:?}", bot.position()))));
    }
    finish_dialog_like(bot, keywords, timeout)
}

/// Talk with fixed answers (Cook-style two-step).
pub fn talk_plain(bot: &mut Bot, name: &str, answers: &[&str], near: Option<(i32, i32)>, timeout: u64) -> Result<(), ScriptError> {
    go_to_npc(bot, name, near)?;
    if bot.world.modal.is_some() {
        bot.close_modal()?;
        bot.pump(5)?;
    }
    if !talk(bot, name, near, timeout)? {
        return Err(ScriptError::Sdk(Error::Unreachable(format!("cannot talk to {name} at {:?}", bot.position()))));
    }
    finish_dialog_exact(bot, answers, timeout)?;
    Ok(())
}

/// True when the quest-complete scroll is on screen.
pub fn completion_open(bot: &Bot) -> bool {
    if bot.world.modal.as_ref().is_none_or(|modal| modal.layer != "main") {
        return false;
    }
    let text: String = bot.world.texts.values().cloned().collect::<Vec<_>>().join(" ").to_lowercase();
    text.contains("you have completed") || text.contains("quest points.")
}

/// Choose any option containing a keyword (else the first), continue pages,
/// close completion scrolls. True when the dialogue closed or completed.
pub fn finish_dialog_like(bot: &mut Bot, keywords: &[&str], timeout: u64) -> Result<(), ScriptError> {
    let target = bot.world.ticks + timeout;
    let mut answered: Vec<String> = Vec::new();
    let mut no_option_since: Option<u64> = None;
    let mut still_texts: Vec<String> = Vec::new();
    let mut still_rounds = 0u32;
    while bot.world.ticks < target {
        let frames = bot.pump(2)?;
        if completion_open(bot) {
            bot.close_modal()?;
            bot.pump(5)?;
            return Ok(());
        }
        let options = bot.chat_options();
        let fresh: Vec<(u16, String)> = options.into_iter().filter(|(_, text)| !answered.contains(text)).collect();
        if !fresh.is_empty() {
            no_option_since = None;
            still_rounds = 0;
            let mut choice: Option<(u16, String)> = None;
            for keyword in keywords {
                let needle = keyword.to_lowercase();
                if let Some(found) = fresh.iter().find(|(_, text)| text.to_lowercase().contains(&needle)) {
                    choice = Some(found.clone());
                    break;
                }
            }
            let picked = choice.unwrap_or_else(|| fresh[0].clone());
            bot.if_button(picked.0)?;
            answered.push(picked.1);
            continue;
        }
        if bot.world.modal.is_some() {
            if completion_open(bot) {
                bot.close_modal()?;
                bot.pump(5)?;
                return Ok(());
            }
            if no_option_since.is_none() {
                no_option_since = Some(bot.world.ticks);
            }
            // A choice page arrives a tick or two after the NPC line: never
            // resume a paused choice, the server falls through to default.
            if bot.world.ticks - no_option_since.unwrap_or(0) < 4 {
                continue;
            }
            if bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "main") {
                bot.close_modal()?;
            } else {
                bot.chat_continue()?;
            }
            no_option_since = None;
            let texts: Vec<String> = {
                let mut all: Vec<String> = bot.world.texts.values().cloned().collect();
                all.sort();
                all
            };
            if texts != still_texts {
                still_texts = texts;
                still_rounds = 0;
            } else if frames.iter().any(|frame| frame.name.starts_with("IF_") || frame.name == "MESSAGE_GAME" || frame.name == "UPDATE_INV_FULL" || frame.name == "UPDATE_INV_PARTIAL") {
                still_rounds = 0;
            } else {
                still_rounds += 1;
            }
            if still_rounds >= 3 {
                // Server already closed the dialogue but its close packet
                // never landed: sync and carry on, verification fails
                // loudly later if this was a mistake.
                bot.close_modal()?;
                bot.pump(5)?;
                return Ok(());
            }
            continue;
        }
        if lingering_closed(bot)? {
            return Ok(());
        }
    }
    Err(ScriptError::Sdk(Error::Timeout(format!("dialog did not close at {:?}", bot.position()))))
}

fn lingering_closed(bot: &mut Bot) -> Result<bool, ScriptError> {
    let linger_until = bot.world.ticks + 150;
    let mut silent = 0u32;
    while bot.world.ticks < linger_until {
        let frames = bot.pump(2)?;
        if bot.world.modal.is_some() || completion_open(bot) {
            return Ok(false);
        }
        if frames.iter().any(|frame| frame.name.starts_with("IF_") || frame.name == "MESSAGE_GAME" || frame.name == "UPDATE_INV_FULL" || frame.name == "UPDATE_INV_PARTIAL") {
            silent = 0;
        } else {
            silent += 1;
        }
        if silent >= 3 {
            return Ok(true);
        }
    }
    Ok(bot.world.modal.is_none() && !completion_open(bot))
}

fn finish_dialog_exact(bot: &mut Bot, answers: &[&str], timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    let mut answered: Vec<String> = Vec::new();
    while bot.world.ticks < target {
        bot.pump(5)?;
        let options: Vec<String> = bot.chat_options().into_iter().map(|(_, text)| text).collect();
        let pending: Vec<&str> = answers.iter().copied().filter(|answer| options.contains(&answer.to_string()) && !answered.contains(&answer.to_string())).collect();
        if !pending.is_empty() {
            let picked = answers.iter().find(|answer| pending.contains(answer)).copied().unwrap_or(pending[0]);
            bot.chat_choose(picked)?;
            answered.push(picked.to_string());
        } else if bot.world.modal.is_some() {
            bot.chat_continue()?;
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Click through plain chat pages until the modal closes or offers a
/// choice. Bounded in ticks; main windows are left for wait_complete.
pub fn drain_dialog(bot: &mut Bot, extra_ticks: u64) -> Result<(), ScriptError> {
    let target = bot.world.ticks + extra_ticks;
    while bot.world.ticks < target {
        if completion_open(bot) {
            return Ok(());
        }
        if bot.world.modal.is_none() {
            if lingering_closed(bot)? {
                return Ok(());
            }
            continue;
        }
        if bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "main") || !bot.chat_options().is_empty() {
            return Ok(());
        }
        bot.chat_continue()?;
        bot.pump(5)?;
    }
    Ok(())
}

/// Operate a loc: approach, click, dismiss the result mesbox.
pub fn operate_loc(bot: &mut Bot, names: &[&str], op: u8, stand: Option<(i32, i32, i32)>) -> Result<(), ScriptError> {
    let (loc_x, loc_z, level, loc) = find_loc(bot, names, 150)?;
    if let Some((stand_x, stand_z, stand_level)) = stand {
        bot.walk_to(stand_x, stand_z, stand_level, 6000)?;
    } else {
        bot.approach(loc_x, loc_z, level, 2, 3000)?;
    }
    bot.op_loc(op, loc_x, loc_z, loc)?;
    wait_ticks(bot, 10)?;
    drain_dialog(bot, 150)
}

/// Use an inventory item on a loc.
pub fn use_item_on_loc(bot: &mut Bot, item_name: &str, loc_names: &[&str]) -> Result<(), ScriptError> {
    let obj = obj_id(item_name)?;
    let (loc_x, loc_z, level, loc) = find_loc(bot, loc_names, 150)?;
    // shortcut: key doors only answer from reach-valid sides; the Rust SDK
    // has no portal table, so try the orthogonal stands nearest-first.
    let mut stands: Vec<(i32, i32)> =
        [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().map(|(dx, dz)| (loc_x + dx, loc_z + dz)).collect();
    stands.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    let seen = bot.world.messages.len();
    for (stand_x, stand_z) in stands {
        if bot.walk_to(stand_x, stand_z, level, 1500).is_err() {
            continue;
        }
        let (use_com, use_slot) = inv_slot(bot, obj)?;
        bot.op_loc_u(loc_x, loc_z, loc, obj, u16::from(use_slot), use_com)?;
        wait_ticks(bot, 10)?;
        if bot.world.messages[seen..].iter().any(|line| line.to_lowercase().contains("can't reach")) {
            continue;
        }
        break;
    }
    wait_ticks(bot, 10)?;
    drain_dialog(bot, 150)
}

/// Use an inventory item on an NPC (research notes on Sedridor).
pub fn use_item_on_npc(bot: &mut Bot, item_name: &str, npc_name: &str, near: Option<(i32, i32)>) -> Result<(), ScriptError> {
    let obj = obj_id(item_name)?;
    go_to_npc(bot, npc_name, near)?;
    let target = bot.world.ticks + 120;
    while bot.world.ticks < target {
        let Some(slot) = bot.find_npc(npc_name) else {
            bot.pump(5)?;
            continue;
        };
        let Some(npc) = bot.world.npcs.get(&slot) else {
            bot.pump(5)?;
            continue;
        };
        let (npc_x, npc_z) = (npc.x, npc.z);
        if bot.approach(npc_x, npc_z, bot.world.level, 2, 1000).is_err() {
            continue;
        }
        let slot = bot.find_npc(npc_name).unwrap_or(slot);
        let (use_com, use_slot) = inv_slot(bot, obj)?;
        bot.op_npc_u(slot, obj, u16::from(use_slot), use_com)?;
        wait_ticks(bot, 10)?;
        drain_dialog(bot, 150)?;
        return Ok(());
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("cannot use {item_name} on {npc_name} at {:?}", bot.position()))))
}

/// Pick up a named ground item, retrying to a tick bound.
pub fn take_near(bot: &mut Bot, name: &str, timeout: u64) -> Result<(), ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        match bot.take(None, Some(name), 1000) {
            Ok(true) => return Ok(()),
            Ok(false) => wait_ticks(bot, 5)?,
            Err(_) => wait_ticks(bot, 5)?,
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("cannot pick up {name} at {:?}", bot.position()))))
}

/// True once the congratulations message or the completion scroll lands.
pub fn wait_complete(bot: &mut Bot, timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if has_message(bot, "congratulations! quest complete!") {
            return Ok(true);
        }
        if completion_open(bot) {
            bot.close_modal()?;
            bot.pump(5)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Approach and attack the nearest named NPC until none is left.
pub fn attack(bot: &mut Bot, names: &[&str], timeout: u64) -> Result<(), ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        let mut foes = bot.npcs_named(names);
        if foes.is_empty() {
            return Ok(());
        }
        foes.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
        let (slot, foe_x, foe_z) = foes[0];
        if (foe_x - bot.world.x).abs() + (foe_z - bot.world.z).abs() > 5
            && bot.approach(foe_x, foe_z, bot.world.level, 2, 1500).is_err()
        {
            bot.pump(5)?;
            continue;
        }
        bot.op_npc(2, slot)?;
        wait_ticks(bot, 5)?;
    }
    if bot.npcs_named(names).is_empty() { Ok(()) } else { Err(ScriptError::Sdk(Error::Timeout(format!("target still alive at {:?}", bot.position())))) }
}

/// Wear an item through its Wield op, verifying the worn container.
pub fn wear(bot: &mut Bot, name: &str) -> Result<(), ScriptError> {
    let obj = obj_id(name)?;
    if worn_count(bot, obj) > 0 {
        return Ok(());
    }
    for _ in 0..6 {
        let Ok((com, slot)) = inv_slot(bot, obj) else { break };
        bot.op_held(2, obj, slot, com)?;
        bot.pump(10)?;
        if worn_count(bot, obj) > 0 {
            return Ok(());
        }
    }
    if worn_count(bot, obj) > 0 {
        Ok(())
    } else {
        Err(ScriptError::Sdk(Error::State(format!("cannot wear {name} at {:?}", bot.position()))))
    }
}

/// Withdraw every requirement item still missing from the backpack.
pub fn withdraw_requirements(bot: &mut Bot, requirements: &QuestRequirements) -> Result<(), ScriptError> {
    for item in &requirements.items {
        let obj = item.obj_id as u16;
        let have = count(bot, obj);
        if have as i64 >= item.count {
            continue;
        }
        bot.bank_withdraw(obj, (item.count - have as i64) as u32, 60)?;
        if (count(bot, obj) as i64) < item.count {
            return Err(ScriptError::Sdk(Error::State(format!("could not withdraw enough {}", item.item))));
        }
    }
    Ok(())
}

/// Walk to the nearest reachable F2P bank booth on this level and open it.
/// Returns the bank contents.
pub fn open_nearest_bank(bot: &mut Bot) -> Result<std::collections::HashMap<u16, u32>, ScriptError> {
    if bot.bank_items().len() > 0 || bot.world.inv.keys().any(|com| {
        gamedata::com_id("bank_main:inv").is_some_and(|bank_com| *com == bank_com)
    }) {
        return Ok(bot.bank_items());
    }
    let mut banks: Vec<gamedata::banks::Bank> = gamedata::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level as i32 == bot.world.level)
        .collect();
    banks.sort_by_key(|bank| (bank.x as i32 - bot.world.x).abs() + (bank.z as i32 - bot.world.z).abs());
    let mut last_error = String::new();
    for bank in banks.iter().take(6) {
        bot.intend(&format!("walking to bank {},{}", bank.x, bank.z));
        if bot.approach(bank.x as i32, bank.z as i32, bank.level as i32, 2, 9000).is_err() {
            last_error = format!("bank {},{} unreachable", bank.x, bank.z);
            continue;
        }
        // Prefer the live booth loc when streamed, else the dump's loc id.
        let booth = find_loc(bot, &["bankbooth"], 15).map(|found| (found.0, found.1, found.3)).unwrap_or((bank.x as i32, bank.z as i32, bank.loc_id as u16));
        match bot.open_bank(booth.0, booth.1, booth.2, 60) {
            Ok(true) => {
                bot.intend("reading the bank");
                return Ok(bot.bank_items());
            }
            Ok(false) => last_error = format!("bank {},{} did not open", bank.x, bank.z),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("no reachable bank, last error: {last_error}"))))
}

/// Report a typed quest result as an `@@EVENT` line for the orchestrator.
pub fn report_result(bot: &Bot, quest: &str, solved: bool, error: Option<&str>) {
    let mut payload = format!("@@EVENT {{\"kind\":\"quest_result\",\"account\":\"{}\",\"quest\":\"{quest}\",\"solved\":{}}}", bot.username, if solved { "true" } else { "false" });
    if let Some(detail) = error {
        payload = payload.trim_end_matches('}').to_string() + &format!(",\"error\":\"{detail}\"}}");
    }
    println!("{payload}");
}

/// Shared requirement gate + solver driver: relogin, junk-free pack, bank
/// top-up, requirement check against live state, solve, completion wait.
// shortcut: save-file completion merge lives in the Python orchestration
// layer; the Rust runner trusts live varps only.
pub fn run_requirements(bot: &mut Bot, requirements: &QuestRequirements, solve: impl FnOnce(&mut Bot) -> Result<(), ScriptError>) -> ScriptResult {
    bot.relogin()?;
    let _ = bot.pump(10)?;
    // Requirement check against live state: skills, carried items, bank.
    if !requirements_met(bot, requirements)? {
        println!("{}: requirements not met", requirements.name);
        report_result(bot, &requirements.key, false, Some("requirements not met"));
        return Ok(0);
    }
    if quest_done(bot, requirements) {
        println!("{}: already complete", requirements.name);
        report_result(bot, &requirements.key, true, None);
        return Ok(0);
    }
    withdraw_requirements(bot, requirements)?;
    bot.close_modal()?;
    bot.pump(5)?;
    match solve(bot) {
        Ok(()) => {
            let solved = wait_complete(bot, 300)? || quest_done(bot, requirements);
            report_result(bot, &requirements.key, solved, None);
            println!("{}: {}", requirements.name, if solved { "completed" } else { "solver ran but no completion message" });
            Ok(0)
        }
        Err(err) => {
            println!("quest solve failed: {err}");
            report_result(bot, &requirements.key, false, Some(&err.to_string()));
            Ok(0)
        }
    }
}

fn requirements_met(bot: &mut Bot, requirements: &QuestRequirements) -> Result<bool, ScriptError> {
    for skill in &requirements.skills {
        let index = skill_index(&skill.skill).unwrap_or(99);
        let level = bot.world.stats.get(&index).map(|(level, _)| *level as i64).unwrap_or(0);
        if level < skill.level {
            return Ok(false);
        }
    }
    let mut carried = bot.inventory();
    for item in &requirements.items {
        let have = carried.get(&(item.obj_id as u16)).copied().unwrap_or(0) as i64 + worn_count(bot, item.obj_id as u16) as i64;
        if have >= item.count {
            continue;
        }
        // Top up from the bank before failing the gate.
        let banked = open_nearest_bank(bot)?;
        carried = bot.inventory();
        let total = carried.get(&(item.obj_id as u16)).copied().unwrap_or(0) as i64
            + worn_count(bot, item.obj_id as u16) as i64
            + banked.get(&(item.obj_id as u16)).copied().unwrap_or(0) as i64;
        if total < item.count {
            return Ok(false);
        }
    }
    Ok(true)
}

fn quest_done(bot: &Bot, requirements: &QuestRequirements) -> bool {
    !requirements.completion_varps.is_empty()
        && requirements.completion_varps.iter().all(|(varp, value)| bot.world.varps.get(&(*varp as u16)).copied().unwrap_or(0) as i64 == *value)
}

fn skill_index(name: &str) -> Option<u8> {
    // Mirrors lifescape SKILL_INDEX order.
    const ORDER: [&str; 21] = [
        "attack", "defence", "strength", "hitpoints", "ranged", "prayer", "magic", "cooking", "woodcutting",
        "fletching", "fishing", "firemaking", "crafting", "smithing", "mining", "herblore", "agility",
        "thieving", "slayer", "farming", "runecraft",
    ];
    ORDER.iter().position(|skill| *skill == name.to_lowercase()).map(|index| index as u8)
}
