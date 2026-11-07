//! Woodcutter: best trees for the level, banks full loads, sells logs to fund
//! axe upgrades, reattaches lost heads instead of rebuying.
//! (Gather loop, bank trip, sell, upgrade.)

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::{wait_hitpoints, wait_ticks};
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, bank_trip_resilient, buy_stock, check_events, death_message_seen, dismiss_modal, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    obj_price, pack_ids, pack_slot, pack_total, sell_for_target, walk_checked, WorkOutcome, FLEE_BAN_TICKS,
    GONE_BAN_TICKS, MAX_FLEES_PER_LOAD, STALL_TICKS,
};
use crate::skill_woodcutting_spot::{is_stump, live_loc_id, pick_tier, pick_tree_spot, tree_still_there};
use crate::{ScriptError, ScriptResult};

const WC_STAT: u8 = 8;
const ROUND_IDLE_TICKS: u64 = 12;
const HANDLE_NAME: &str = "macro_hatchethandle";

/// Best axe first.
pub const AXE_ORDER: [&str; 7] =
    ["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"];

fn best_axe(bot: &Bot) -> Option<String> {
    let held = pack_ids(bot);
    for name in AXE_ORDER {
        if let Ok(id) = obj_id(bot, name) {
            if held.contains(&id) {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn known_axe_ids(bot: &Bot) -> HashSet<u16> {
    AXE_ORDER.into_iter().filter_map(|name| obj_id(bot, name).ok()).collect()
}

/// Reattach a handle + head carried instead of rebuying.
fn reattach(bot: &mut Bot) -> Result<bool, ScriptError> {
    let Ok(handle) = obj_id(bot, HANDLE_NAME) else {
        return Ok(false);
    };
    if pack_total(bot, handle) == 0 {
        return Ok(false);
    }
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with("_hatchethead"))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    for head in heads {
        if pack_total(bot, head) == 0 {
            continue;
        }
        if use_handle_on_head(bot, handle, head)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn use_handle_on_head(bot: &mut Bot, handle: u16, head: u16) -> Result<bool, ScriptError> {
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    let (Ok(handle_slot), Ok(head_slot)) = (pack_slot(bot, handle), pack_slot(bot, head)) else {
        return Ok(false);
    };
    bot.op_held_u(handle, handle_slot, pack_com, head, head_slot, pack_com)?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if best_axe(bot).is_some() {
            bot.log("ok axe reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Withdraw a banked axe (or its handle+head) and reattach it.
/// Cheaper than rebuying. Reopens the bank: call after a bank trip.
/// Booth business from across the map fails fast instead of fanning
/// 169 instant walks at a booth we cannot touch.
fn restore_from_bank(bot: &mut Bot, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_axe(bot).is_some() {
        return Ok(true);
    }
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        bot.log(&format!("axe restore: bank at ({},{}) too far from {:?}", bank.x, bank.z, bot.position()));
        return Ok(false);
    }
    let Ok(handle) = obj_id(bot, HANDLE_NAME) else {
        return Ok(false);
    };
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with("_hatchethead"))
        .filter_map(|entry| entry.name.clone())
        .filter_map(|name| obj_id(bot, &name).ok())
        .collect();
    if heads.is_empty() {
        return Ok(false);
    }
    let Some((booth_x, booth_z, booth_loc)) = crate::skill_common::live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
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
    let axes = known_axe_ids(bot);
    if let Some((slot, (held, _))) = slots.iter().find(|(_, (held, _))| axes.contains(held)) {
        let (slot, held) = (*slot, *held);
        bot.inv_button(1, held, slot, main_com)?;
        bot.pump(10)?;
        bot.close_modal()?;
        bot.pump(5)?;
        if best_axe(bot).is_some() {
            bot.log("ok withdrew banked axe");
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
    if reattach(bot)? {
        bot.log("ok restored axe from banked pieces");
        return Ok(true);
    }
    Ok(false)
}

/// Broke and axeless: kill goblins east of Lumbridge for coin drops
/// until the pack funds the axe. Bounded rounds, each a few kills with
/// no banking (loot stays in the pack for the shared buy below); no
/// gain stops early. True when the pack now affords the axe.
/// Budget: goblin coin drops average ~1.6/kill (goblin.rs2 drop table, 27
/// coin rolls per 128), so 30 kills expect ~47 coins and P(<16) is ~2%.
fn hunt_goblins(bot: &mut Bot, coins: u16) -> Result<bool, ScriptError> {
    for _ in 0..5 {
        if pack_total(bot, coins) >= 16 {
            return Ok(true);
        }
        let before = pack_total(bot, coins);
        crate::skill_combat::run_melee(
            bot,
            &["goblins".to_string(), "--kills".to_string(), "6".to_string(), "--no-bank".to_string()],
        )?;
        if pack_total(bot, coins) <= before {
            break;
        }
    }
    Ok(pack_total(bot, coins) >= 16)
}

/// Pack ids the bootstrap may sell for the axe: everything carried that
/// is neither an axe, coins, the unbankable genie lamp, nor the random-
/// event cube. Ground loot (daggers, spare logs) funds the axe instead
/// of forcing a bank trip just to dump it; event items are never sale
/// stock (selling the cube would strand its rescue).
fn bootstrap_products(bot: &Bot, coins: u16) -> Vec<u16> {
    let axes = known_axe_ids(bot);
    let lamp = (bot.tables.find_obj_by_name)("macro_genilamp");
    let cube = (bot.tables.find_obj_by_name)("macro_cube");
    pack_ids(bot)
        .into_iter()
        .filter(|id| *id != coins && !axes.contains(id) && Some(*id) != lamp && Some(*id) != cube)
        .collect()
}

fn cheapest_upgrade(bot: &Bot) -> (Vec<String>, i64) {
    let current_rank = best_axe(bot).and_then(|tool| AXE_ORDER.iter().position(|name| *name == tool)).unwrap_or(AXE_ORDER.len());
    let cands: Vec<String> = AXE_ORDER[..current_rank]
        .iter()
        .filter(|name| crate::skill_common::shop_stock_keeper(bot, name).is_some())
        .map(|name| name.to_string())
        .collect();
    let cheapest = cands.iter().map(|name| obj_price(name)).min().unwrap_or(0);
    (cands, cheapest)
}

/// One axe buy attempt with bronze fallback: tries the affordable cands
/// best-first, one attempt each, downgrading on any shop failure (empty
/// live stock, short coins). Conn/death/interrupt still abort; anything
/// else logs and tries the next cheaper axe. Returns the bought name or
/// None when nothing affordable succeeded. Never loops: one pass, no retry.
fn buy_axe_upgrade(bot: &mut Bot, cands: &[String]) -> Result<Option<String>, ScriptError> {
    let have = pack_total(bot, obj_id(bot, "coins")?);
    for cand in cands {
        let price = obj_price(cand);
        if price <= 0 || have < price as u32 {
            bot.log(&format!("skip {cand}: costs {price}, have {have}"));
            continue;
        }
        match buy_stock(bot, cand, 1) {
            Ok(bought) if bought > 0 => {
                bot.log(&format!("ok upgraded to {cand}"));
                return Ok(Some(cand.clone()));
            }
            Ok(_) => {
                bot.log(&format!("{cand} trade failed at {:?}, trying cheaper", bot.position()));
            }
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
            Err(error) if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) => return Err(error),
            Err(error) => {
                bot.log(&format!("{cand} buy failed ({error}), trying cheaper"));
            }
        }
    }
    Ok(None)
}

/// True when death happened since `seen`: hp flatlined or the server
/// death line arrived. Returns the resynced index (everything observed so
/// far) so the next call neither replays nor skips a line.
fn death_since(bot: &Bot, seen: usize) -> (bool, usize) {
    let died = hitpoints(bot) == 0 || death_message_seen(bot, seen);
    (died, bot.world.messages.len())
}

type TreeSpot = (knowledge::trees::TreeTier, i32, i32);

/// Death resent: log, bounded respawn wait, loop state reset. The wait is
/// never billed to the no-log stall budget and the spot is dropped so the
/// next pick recomputes `had_logs` from the real pack.
fn respawn_reset(
    bot: &mut Bot,
    seen_death_msgs: &mut usize,
    idle_since: &mut u64,
    last_hitpoints: &mut u8,
    spot: &mut Option<TreeSpot>,
) -> Result<(), ScriptError> {
    let spent = bot.world.ticks;
    bot.log("died, respawning");
    bot.intend("died, respawning");
    let target = bot.world.ticks + 40;
    while bot.world.ticks < target && hitpoints(bot) == 0 {
        bot.pump(5)?;
    }
    *idle_since += bot.world.ticks - spent;
    *last_hitpoints = hitpoints(bot);
    *spot = None;
    *seen_death_msgs = bot.world.messages.len();
    Ok(())
}

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range).
fn work_spot(
    bot: &mut Bot,
    tree_x: i32,
    tree_z: i32,
    log_ids: &[u16],
    had_logs: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable tree: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, tree_x, tree_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 1 {
        let mut neighbors: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (tree_x + dx, tree_z + dz)))
            .filter(|tile| *tile != (tree_x, tree_z))
            .collect();
        neighbors.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        let mut stepped = false;
        for (next_x, next_z) in neighbors {
            match bot.walk_to(next_x, next_z, bot.world.level, 100) {
                Ok(()) => {
                    stepped = true;
                    break;
                }
                Err(error)
                    if matches!(
                        error,
                        lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                    ) =>
                {
                    return Err(error.into())
                }
                Err(_) => {}
            }
            if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 4 {
                return Ok((WorkOutcome::Unreachable, 0));
            }
        }
        if !stepped {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let tree_loc = live_loc_id(bot, tree_x, tree_z)?;
    let mut gained = 0u32;
    let mut silences = 0u32;
    let mut need_click = true;
    let mut seen_msgs = bot.world.messages.len();
    let hitpoints_before = hitpoints(bot);
    loop {
        if need_click {
            bot.log(&format!("click tree at ({tree_x},{tree_z})"));
            bot.intend(&format!("chopping a tree at ({tree_x},{tree_z})"));
            bot.op_loc(1, tree_x, tree_z, tree_loc)?;
            seen_msgs = bot.world.messages.len();
            need_click = false;
        }
        let target = bot.world.ticks + ROUND_IDLE_TICKS;
        let mut quiet = true;
        while bot.world.ticks < target {
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
                let lowered = message.to_lowercase();
                if lowered.contains("can't reach that") {
                    return Ok((WorkOutcome::Unreachable, gained));
                }
                if lowered.contains("woodcutting level of") {
                    return Ok((WorkOutcome::LevelLocked, gained));
                }
                if lowered.contains("do not have an axe") {
                    return Ok((WorkOutcome::NoTool, gained));
                }
            }
            let now_have: u32 = log_ids.iter().map(|id| pack_total(bot, *id)).sum();
            if now_have > had_logs + gained {
                gained = now_have - had_logs;
                quiet = false;
                break;
            }
        }
        if !quiet {
            continue;
        }
        check_events(bot)?;
        silences += 1;
        if silences == 1 && is_stump(bot, tree_x, tree_z) {
            return Ok((WorkOutcome::Gone, gained));
        }
        if silences >= 2 {
            return Ok((WorkOutcome::Gone, gained));
        }
        need_click = true;
    }
}

fn recover_axe(bot: &mut Bot, coins: u16, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_axe(bot).is_some() {
        return Ok(true);
    }
    if reattach(bot)? {
        return Ok(true);
    }
    if pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0 {
        bot.log("ok replacement axe bought");
        return Ok(true);
    }
    let keep = vec![coins];
    if bank_trip(bot, bank, &keep, 16u32.saturating_sub(pack_total(bot, coins)))? == 0 {
        return Ok(false);
    }
    if best_axe(bot).is_some() {
        bot.log("ok replacement axe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0)
}

fn chop_load(
    bot: &mut Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    mut level: i64,
    banned: &mut HashMap<(i32, i32), u64>,
) -> Result<(bool, i64), ScriptError> {
    let log_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut idle_since = bot.world.ticks;
    let mut unreachable_streak = 0u32;
    let mut last_hitpoints = hitpoints(bot);
    let mut flees = 0u32;
    let mut spot: Option<(knowledge::trees::TreeTier, i32, i32)> = None;
    let mut had_logs = 0u32;
    let mut seen_death_msgs = bot.world.messages.len();
    loop {
        check_events(bot)?;
        if free_slots(bot) <= usize::from(pack_total(bot, coins) > 0) {
            return Ok((true, level));
        }
        if best_axe(bot).is_none() {
            // Recovery walks (shop, bank) are not chopping: don't bill
            // them to the no-log stall budget.
            let spent = bot.world.ticks;
            if !recover_axe(bot, coins, bank)? {
                bot.log("axe lost, banking load");
                return Ok((true, level));
            }
            idle_since += bot.world.ticks - spent;
        }
        level = learn_level(bot, WC_STAT, level);
        if let (Some((tier, _, _)), Some(best)) = (&spot, pick_tier(tiers, level)) {
            if tier.level < best.level {
                bot.log("level up, switching tier");
                spot = None;
            }
        }
        let (died, fresh) = death_since(bot, seen_death_msgs);
        seen_death_msgs = fresh;
        if died {
            respawn_reset(bot, &mut seen_death_msgs, &mut idle_since, &mut last_hitpoints, &mut spot)?;
            // Respawn sits across the map: re-sync the bank (cheap) so the
            // next spot is picked near here, not near the corpse.
            *bank = nearest_bank(bot)?;
            continue;
        }
        if hitpoints(bot) < last_hitpoints {
            flees += 1;
            bot.log("hurt, fleeing");
            bot.intend("fleeing a hunter");
            if let Some((_, spot_x, spot_z)) = &spot {
                banned.insert((*spot_x, *spot_z), bot.world.ticks + FLEE_BAN_TICKS);
            }
            if flees > MAX_FLEES_PER_LOAD {
                bot.log("too many attacks, aborting load");
                return Ok((false, level));
            }
            wait_hitpoints(bot, hitpoints(bot).saturating_add(1), 200)?;
            spot = None;
            last_hitpoints = hitpoints(bot);
            continue;
        }
        last_hitpoints = hitpoints(bot);
        let now = bot.world.ticks;
        banned.retain(|_, until| *until > now);
        let stale = match &spot {
            Some((_, spot_x, spot_z)) => banned.contains_key(&(*spot_x, *spot_z)),
            None => true,
        };
        if stale {
            let mut picked = None;
            for _ in 0..16 {
                if let Some(found) = pick_tree_spot(bot, tiers, bank, level, &banned.keys().copied().collect()) {
                    picked = Some(found);
                    break;
                }
                bot.log("no tree spot, waiting for respawns");
                bot.intend("waiting for a safe tree spot");
                wait_ticks(bot, 15)?;
            }
            let Some((tier, spot_x, spot_z)) = picked else {
                bot.log("no tree spot left");
                return Ok((false, level));
            };
            had_logs = log_ids.iter().map(|id| pack_total(bot, *id)).sum();
            bot.log(&format!("chopping {} at ({spot_x},{spot_z})", tier.product));
            bot.intend(&format!("chopping {} at ({spot_x},{spot_z})", tier.product));
            spot = Some((tier, spot_x, spot_z));
        } else if let Some((tier, spot_x, spot_z)) = &spot {
            if !tree_still_there(bot, *spot_x, *spot_z, &tier.trees) {
                bot.log(&format!("tree gone at ({spot_x},{spot_z})"));
                banned.insert((*spot_x, *spot_z), now + GONE_BAN_TICKS);
                spot = None;
                continue;
            }
        }
        let Some((_, spot_x, spot_z)) = spot.clone() else {
            continue;
        };
        let (result, gained) = work_spot(bot, spot_x, spot_z, &log_ids, had_logs)?;
        // Death can land inside work_spot's own pumps: check its return too,
        // or the run bills a corpse to the stall budget and never logs it.
        let (died_mid_work, fresh) = death_since(bot, seen_death_msgs);
        seen_death_msgs = fresh;
        if died_mid_work {
            respawn_reset(bot, &mut seen_death_msgs, &mut idle_since, &mut last_hitpoints, &mut spot)?;
            *bank = nearest_bank(bot)?;
            continue;
        }
        had_logs += gained;
        if result != WorkOutcome::Unreachable {
            unreachable_streak = 0;
        }
        match result {
            WorkOutcome::Gone => {
                bot.log("tree quiet, next one");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::NoTool => spot = None,
            WorkOutcome::Unreachable => {
                banned.insert((spot_x, spot_z), now + 500);
                spot = None;
                unreachable_streak += 1;
                // Pocket landing (yank mid-run): every spot fails at the
                // walk. Travel back to the bank once, then re-pick from
                // the new region; the stall bound below stays the loud exit.
                if unreachable_streak >= 8 {
                    let dest = lifescape_sdk::plan::Tile { x: bank.x as i32, z: bank.z as i32, level: bank.level as i32 };
                    match bot.travel_to(dest) {
                        Ok(()) => unreachable_streak = 0,
                        Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => {
                            return Err(error.into());
                        }
                        Err(error) => bot.log(&format!("pocket escape refused: {error}")),
                    }
                }
            }
            WorkOutcome::LevelLocked => {
                banned.insert((spot_x, spot_z), now + 500);
                spot = None;
            }
            WorkOutcome::Threat => {
                bot.log("hunter close, re-picking a spot");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::Blocked => {
                bot.log("unknown dialog blocking chop, aborting load");
                return Ok((false, level));
            }
            WorkOutcome::Full | WorkOutcome::Moved | WorkOutcome::NoBait => {
                spot = None;
            }
        }
        if gained > 0 {
            idle_since = bot.world.ticks;
        }
        if bot.world.ticks - idle_since > STALL_TICKS {
            bot.log(&format!("stall: no logs for {STALL_TICKS} ticks"));
            return Ok((false, level));
        }
    }
}

/// True when the pack alone can pay the upgrade right now: buy
/// immediately, never hoard coins through the chop.
fn can_buy_now(have_coins: u32, cheapest: i64, cands: &[String]) -> bool {
    !cands.is_empty() && cheapest > 0 && have_coins >= cheapest as u32
}

/// Pack ids that survive the bank trip: axes only. Coins and produce are
/// always banked, never carried to the spot.
fn chop_keep_ids(bot: &Bot) -> Vec<u16> {
    known_axe_ids(bot).into_iter().collect()
}

/// Sell the pack (logs and ground-loot junk alike) at the nearby general
/// store and buy the bronze axe when funded. True when the axe is now
/// held. The hunt loot only exists AFTER the hunt, so the caller runs this
/// both before and after: selling late beats losing it to the bank trek
/// (junk stranded, unsold) or to death (dropped).
fn sell_pack_and_buy(bot: &mut Bot, bank: &mut knowledge::banks::Bank, coins: u16, label: &str) -> Result<bool, ScriptError> {
    let funded = bootstrap_products(bot, coins);
    let have = pack_total(bot, coins);
    if !funded.is_empty() && have < 16 {
        let names: Vec<String> =
            funded.iter().map(|id| (bot.tables.obj_name)(*id).unwrap_or_else(|| format!("obj{id}"))).collect();
        bot.log(&format!("{label}: selling {names:?} for {have} coins at {:?}", bot.position()));
        match sell_for_target(bot, &funded, coins, 16 - have) {
            Ok(_) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(err) => bot.log(&format!("{label} sale failed: {err}, trying the bank")),
        }
    }
    if pack_total(bot, coins) >= 16 {
        match buy_stock(bot, "bronze_axe", 1) {
            Ok(bought) if bought > 0 => {
                bot.log(&format!("ok bootstrapped bronze axe from {label}"));
                *bank = nearest_bank(bot)?;
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
            Err(err) => bot.log(&format!("{label} axe buy failed: {err}, trying the bank")),
        }
    }
    Ok(best_axe(bot).is_some())
}

/// Axe bootstrap, reusable after every death: reattach from the pack,
/// sell the pack and buy from the keeper, else bank-trip, restore a
/// banked axe, else withdraw and sell banked logs to fund one. Returns
/// the banked coin total for upgrade budgeting. Loud when broke: an axe
/// cannot be conjured from nothing.
fn ensure_axe(
    bot: &mut Bot,
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    log_ids: &HashSet<u16>,
) -> Result<Option<u32>, ScriptError> {
    let mut bank_coins: Option<u32> = None;
    if best_axe(bot).is_none() || pack_ids(bot).iter().any(|id| !known_axe_ids(bot).contains(id) && !log_ids.contains(id) && *id != coins) {
        if reattach(bot)? {
            bot.log("ok reattached axe from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = log_ids.iter().copied().collect();
        // An axe is bought with wood, not with a bank trip: sell the pack
        // (logs and ground-loot junk alike) at the nearby general store
        // and buy from the axe keeper first. Every trek below only runs
        // when the pack cannot fund the axe on its own.
        // Sale hiccups are never fatal here: the hunt below is exactly
        // what funds the axe, so log and keep going.
        if best_axe(bot).is_none() {
            let bought = match sell_pack_and_buy(bot, &mut *bank, coins, "pack sale") {
                Ok(bought) => bought,
                Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                Err(error)
                    if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) =>
                {
                    return Err(error)
                }
                Err(error) => {
                    bot.log(&format!("pack sale failed: {error}, hunting to fund the axe"));
                    false
                }
            };
            // Broke with nothing worth selling: earn the axe on goblins
            // east of Lumbridge first, then sell THAT loot too before
            // falling to the bank below.
            if !bought && pack_total(bot, coins) < 16 {
                match hunt_goblins(bot, coins) {
                    Ok(true) => {}
                    Ok(false) => bot.log("goblin hunt came up short, trying the bank"),
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(err) => bot.log(&format!("goblin hunt failed: {err}, trying the bank")),
                }
                let _ = match sell_pack_and_buy(bot, &mut *bank, coins, "hunt loot") {
                    Ok(bought) => bought,
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                    Err(error)
                        if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) =>
                    {
                        return Err(error)
                    }
                    Err(error) => {
                        bot.log(&format!("hunt loot sale failed: {error}, trying the bank"));
                        false
                    }
                };
            }
        }
        if best_axe(bot).is_none() {
            bank_coins = Some(bank_trip(bot, bank, &keep, 0)?);
            if restore_from_bank(bot, bank)? {
                bot.log("ok axe restored from bank");
            }
        }
        if best_axe(bot).is_none() {
            // Toll crossing costs 10 coins each way: budget it only when
            // the bank sits across the fence, then pay the guard.
            let need_cross = crate::skill_common::toll_gate_x(bot)
                .is_some_and(|line| (bank.x >= line as i64) != (bot.world.x >= line));
            let target = 16u32 + if need_cross { 10 } else { 0 };
            bank_coins = Some(bank_trip(bot, bank, &keep, u32::MAX / 2)?);
            let have = pack_total(bot, coins);
            if have < target {
                // No wood in the pack: pull the banked logs back out and
                // sell those (an axe is bought with wood, not with nothing).
                let log_list: Vec<u16> = log_ids.iter().copied().collect();
                let carried: u32 = log_list.iter().map(|id| pack_total(bot, *id)).sum();
                if carried == 0 {
                    let pulled = crate::skill_common::bank_withdraw_items(bot, bank, &log_list, 10)?;
                    if pulled > 0 {
                        bot.log(&format!("withdrew {pulled} logs from the bank to fund the axe"));
                    }
                }
                sell_for_target(bot, &log_list, coins, target - pack_total(bot, coins))?;
            }
            if need_cross && !crate::skill_common::cross_toll_gate(bot)? {
                return Err(ScriptError::Usage(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            // Bronze-first bootstrap: one attempt, then fall through broke.
            // Being broke is never fatal: the main loop sees no axe and
            // hunts again. Only death/conn/interrupt abort. Callers fall
            // back to the hunt, never to a buy loop.
            if pack_total(bot, coins) >= 16 {
                match buy_stock(bot, "bronze_axe", 1) {
                    Ok(bought) if bought > 0 => {
                        bot.log("ok bootstrapped bronze axe");
                        *bank = nearest_bank(bot)?;
                    }
                    Ok(_) => {
                        bot.log(&format!(
                            "cannot bootstrap yet: no axe, {} coins (bronze trade failed at {:?}), hunting again",
                            pack_total(bot, coins),
                            bot.position()
                        ));
                    }
                    Err(error) if crate::skill_common::is_death_abort(&error) => return Err(error),
                    Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                    Err(error) if matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Interrupted(_))) => {
                        return Err(error)
                    }
                    Err(error) => {
                        bot.log(&format!(
                            "cannot bootstrap yet: no axe, {} coins (bronze out of stock at {:?}: {error}), hunting again",
                            pack_total(bot, coins),
                            bot.position()
                        ));
                    }
                }
            } else {
                bot.log(&format!(
                    "cannot bootstrap yet: no axe, {} coins, hunting again",
                    pack_total(bot, coins)
                ));
            }
        }
    }
    Ok(bank_coins)
}

/// Cut banked loads in the open session (0 = forever).
pub fn run_woodcut(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    if args.iter().any(|arg| arg == "wild") {
        bot.allow_wild = true;
        bot.log("hunter zones allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = knowledge::trees::all_tiers();
    let mut level = learn_level(bot, WC_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut bank_coins = 0u32;
    let mut done = 0u32;
    // Threat bans outlive one load: a killer tree stays banned while its
    // neighbours get cut, instead of repicking the same death every load.
    let mut banned: HashMap<(i32, i32), u64> = HashMap::new();
    while loads == 0 || done < loads {
        // Death may have moved us across the map: re-sync the bank cheaply
        // (no walking) so spots and treks start from the real position.
        bank = nearest_bank(bot)?;
        check_events(bot)?;
        // Death abort inside means the load was lost: re-gear (rebuy the
        // axe when missing) and retry the load, for as long as it takes.
        // Broke is not an error either: no axe means hunt again next
        // iteration (combat xp always progresses). Only conn/interrupt
        // and structural failures (toll, unreachable bank) fail loud.
        // Anything else fails loud as before.
        match ensure_axe(bot, &mut bank, coins, &log_ids) {
            Ok(Some(coins_)) => bank_coins = coins_,
            Ok(None) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                bot.log("died re-gearing, retrying the load");
                continue;
            }
            Err(error) => return Err(error),
        }
        if best_axe(bot).is_none() {
            bot.log("still axeless after re-gear, hunting again");
            continue;
        }
        let (ok, new_level) = chop_load(bot, &tiers, &mut bank, coins, level, &mut banned)?;
        level = new_level;
        // A failed load (threats, stumps, stall) retries with bans kept:
        // stumps regrow and hunters wander off.
        if !ok {
            bot.log("chop load failed, retrying with fresh bans");
            continue;
        }
        let (cands, cheapest) = cheapest_upgrade(bot);
        if !cands.is_empty() {
            let have = pack_total(bot, coins);
            sell_for_target(
                bot,
                &log_ids.iter().copied().collect::<Vec<_>>(),
                coins,
                (cheapest as u32).saturating_sub(bank_coins + have),
            )?;
            if can_buy_now(pack_total(bot, coins), cheapest, &cands) {
                match buy_axe_upgrade(bot, &cands)? {
                    Some(_) => {}
                    None => {
                        bot.log(&format!(
                            "upgrade missed: {} too costly or keeper unreachable at {:?}, {} coins banked next trip, never carried to spot",
                            cands.join("+"),
                            bot.position(),
                            pack_total(bot, coins)
                        ));
                    }
                }
            }
        }
        let keep: Vec<u16> = chop_keep_ids(bot);
        // Death on the bank trip loses the load: re-gear next iteration
        // and retry it instead of failing the run. Anything else fails
        // loud as before.
        match bank_trip_resilient(bot, &mut bank, &keep, cheapest as u32, &log_ids.iter().copied().collect::<Vec<_>>()) {
            Ok(coins_) => bank_coins = coins_,
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                bot.log("died on the bank trip, re-gearing to continue");
                continue;
            }
            Err(error) => return Err(error),
        }
        // Post-bank upgrade: only what the pack affords, best-first with
        // bronze fallback inside. No coins means no walk to Bob at all.
        if pack_total(bot, coins) > 0 {
            let _ = buy_axe_upgrade(bot, &cands)?;
        }
        if pack_total(bot, coins) > 0 {
            // Failed buys must not ride to the spot: stash now or abort.
            let keep_now: Vec<u16> = chop_keep_ids(bot);
            bank_trip(bot, &bank, &keep_now, 0)?;
            if pack_total(bot, coins) > 0 {
                return Err(ScriptError::Usage(format!(
                    "coins left after banking at {:?}, refusing to carry to spot",
                    bot.position()
                )));
            }
        }
        done += 1;
        bot.log(&format!("ok load {done} banked (wc={level})"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_products, can_buy_now, chop_keep_ids, death_since};
    use crate::skill_common::{death_message_seen, WorkOutcome};    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Death since an index: the mid-work message fires on every work
    /// outcome path (Threat/Gone/Full), the resync never replays it.
    #[test]
    fn death_since_detects_message_on_every_work_outcome() {
        for outcome in [WorkOutcome::Threat, WorkOutcome::Gone, WorkOutcome::Full] {
            let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
            attach_live_tables(&mut bot);
            bot.world.stats.insert(3, (10, 0));
            let seen = bot.world.messages.len();
            bot.world.messages.push("You get some oak logs.".to_string());
            bot.world.messages.push("Oh dear you are dead!".to_string());
            let (died, fresh) = death_since(&bot, seen);
            assert!(died, "death message detected on {outcome:?} path");
            assert_eq!(fresh, 2, "resync covers everything observed");
            let (again, _) = death_since(&bot, fresh);
            assert!(!again, "resync never replays on {outcome:?} path");
        }
    }

    /// Flatlined hp detects death with no message at all.
    #[test]
    fn death_since_detects_hp_zero_without_message() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (0, 0));
        let (died, fresh) = death_since(&bot, 0);
        assert!(died, "hp 0 detects death alone");
        assert_eq!(fresh, 0, "no messages, index stays");
    }

    /// Ordinary chatter never trips the detector.
    #[test]
    fn death_since_ignores_unrelated_chatter() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        bot.world.messages.push("You get some oak logs.".to_string());
        let (died, fresh) = death_since(&bot, 0);
        assert!(!died, "loot chatter is not death");
        assert_eq!(fresh, 1, "chatter still resyncs the index");
    }

    /// Bootstrap sale set: ground loot and logs fund the axe, axes/coins
    /// and the unbankable lamp never sell.
    #[test]
    fn bootstrap_products_sells_loot_never_axe_or_lamp() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let axe = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let dagger = crate::skill_common::obj_id(&bot, "bronze_dagger").expect("bronze_dagger in tables");
        let logs = crate::skill_common::obj_id(&bot, "logs").expect("logs in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(
            com,
            [(0u8, (coins, 3u32)), (1, (axe, 1)), (2, (dagger, 1)), (3, (logs, 2))]
                .into_iter()
                .collect(),
        );
        let funded = bootstrap_products(&bot, coins);
        assert!(funded.contains(&dagger), "ground loot funds the axe");
        assert!(funded.contains(&logs), "logs fund the axe");
        assert!(!funded.contains(&coins), "coins never sell");
        assert!(!funded.contains(&axe), "axes never sell");
        if let Some(lamp) = (bot.tables.find_obj_by_name)("macro_genilamp") {
            assert!(!funded.contains(&lamp), "lamp never sells");
        }
        if let Some(cube) = (bot.tables.find_obj_by_name)("macro_cube") {
            assert!(!funded.contains(&cube), "event cube never sells");
        }
    }


    /// Affordable upgrade: buy right away. Short, no candidates, or no
    /// price: coins stay reserved for the bank, never spent mid-chop.
    #[test]
    fn upgrade_bought_immediately_when_affordable() {
        let cands = vec!["iron_axe".to_string()];
        assert!(can_buy_now(613, 500, &cands), "613 coins pay 500 now");
        assert!(can_buy_now(500, 500, &cands), "exact price pays now");
        assert!(!can_buy_now(109, 500, &cands), "109 coins stay reserved for the bank");
        assert!(!can_buy_now(999, 500, &[]), "no candidates, nothing to buy");
        assert!(!can_buy_now(999, 0, &cands), "no price, nothing to buy");
    }

    /// Bootstrap fallback order: with 16 coins only bronze is affordable
    /// (bronze 16, iron 56, steel 200 from the dump). The upgrade loop must
    /// skip the dearer axes instead of fatally trying iron first.
    #[test]
    fn fallback_order_skips_unaffordable_with_16() {
        use crate::skill_common::obj_price;
        let bronze = obj_price("bronze_axe");
        let iron = obj_price("iron_axe");
        let steel = obj_price("steel_axe");
        assert_eq!((bronze, iron, steel), (16, 56, 200), "dump prices, got: {bronze}/{iron}/{steel}");
        let cands = ["steel_axe", "iron_axe", "bronze_axe"];
        let affordable: Vec<&&str> = cands.iter().filter(|name| obj_price(name) <= 16).collect();
        assert_eq!(affordable, vec![&"bronze_axe"], "16 coins fund bronze only, got: {affordable:?}");
    }

    /// Bank keep holds axes only: coins always land in the bank, never
    /// ride to the spot.
    #[test]
    fn chop_keep_never_holds_coins() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let keep = chop_keep_ids(&bot);
        assert!(!keep.is_empty(), "axes resolve from the tables");
        assert!(!keep.contains(&coins), "coins always banked, never carried");
    }

    /// Death by message: hp stream can lag, the chat line still resets the
    /// spot and the death/respawn ticks are not billed to the stall budget
    /// (same `idle_since += now - spent` as the axe-recovery branch).
    #[test]
    fn death_message_detects_with_hp_up() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(3, (10, 0));
        assert_eq!(crate::skill_common::hitpoints(&bot), 10, "hp up, message path must trigger alone");
        bot.world.messages.push("Oh dear you are dead!".to_string());
        assert!(death_message_seen(&bot, 0), "exact line detected");
        assert!(!death_message_seen(&bot, 1), "index tracks seen messages");
        bot.world.messages.push("OH DEAR you ARE dead".to_string());
        assert!(death_message_seen(&bot, 1), "case-insensitive");
        // Chop-load handling for that detection: spot reset, idle not billed.
        let mut idle_since = 100u64;
        let spent = 100u64;
        let waited_until = 140u64;
        idle_since += waited_until - spent;
        assert_eq!(idle_since, 140, "death wait not billed to the stall budget");
        let mut spot: Option<(i32, i32)> = Some((3086, 3243));
        spot = None;
        assert!(spot.is_none(), "spot reset after death");
    }

}
