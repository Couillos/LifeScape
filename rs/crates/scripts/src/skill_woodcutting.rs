//! Woodcutter: best trees for the level, banks full loads, sells logs to fund
//! axe upgrades, reattaches lost heads instead of rebuying.
//! Same shape as `scripts/woodcut/run.py`.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::{wait_hitpoints, wait_ticks};
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, check_events, dismiss_modal, free_slots, hitpoints, learn_level, nearest_bank, obj_id,
    obj_price, pack_ids, pack_slot, pack_total, sell_for_target, walk_checked, WorkOutcome, FLEE_BAN_TICKS,
    GONE_BAN_TICKS, MAX_FLEES_PER_LOAD, STALL_TICKS,
};
use crate::{ScriptError, ScriptResult};

const WC_STAT: u8 = 8;
const NEAR_BANK_TILES: i64 = 150;
const ROUND_IDLE_TICKS: u64 = 12;
const HANDLE_NAME: &str = "macro_hatchethandle";

/// Best axe first (mirrors the Python AXE_ORDER).
pub const AXE_ORDER: [&str; 7] =
    ["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"];

fn pick_tier(tiers: &[knowledge::trees::TreeTier], level: i64) -> Option<knowledge::trees::TreeTier> {
    let mut best: Option<&knowledge::trees::TreeTier> = None;
    for tier in tiers {
        if tier.level <= level && best.as_ref().is_none_or(|known| tier.level > known.level) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Tree spawns for the tier's loc names within range of the bank.
fn tier_spawns(tier: &knowledge::trees::TreeTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.trees.iter().any(|tree| *tree == name) {
            for spawn in &entry.spawns {
                if spawn.level == bank.level
                    && (spawn.x - bank.x).abs() + (spawn.z - bank.z).abs() <= NEAR_BANK_TILES
                {
                    found.push((spawn.x as i32, spawn.z as i32));
                }
            }
        }
    }
    found
}

/// False when the server cleared this tile or confirmed something else
/// here. A tile that has not streamed yet counts as standing (the static
/// template is the truth until the server says otherwise).
fn tree_live(bot: &Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    match bot.world.loc_at(spot_x, spot_z, bot.world.level) {
        Some((loc, _, _)) => knowledge::loc_by_id(usize::from(loc))
            .is_none_or(|entry| entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase()))),
        None => true,
    }
}

fn pick_tree_spot(
    bot: &Bot,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &HashSet<(i32, i32)>,
) -> Option<(knowledge::trees::TreeTier, i32, i32)> {
    let mut affordable: Vec<&knowledge::trees::TreeTier> = tiers.iter().filter(|tier| tier.level <= level).collect();
    affordable.sort_by_key(|tier| -tier.level);
    for tier in affordable {
        let mut best: Option<(i64, (i32, i32))> = None;
        for (spot_x, spot_z) in tier_spawns(tier, bank) {
            if banned.contains(&(spot_x, spot_z)) || !tree_live(bot, spot_x, spot_z, &tier.trees) {
                continue;
            }
            // shortcut: no hunter table in the gamedata dump (Npc carries no
            // hunts_players flag), so spots skip the static safety filter;
            // hurt-driven flee still relocates mid-load.
            let dist = i64::from((spot_x - bot.world.x).abs() + (spot_z - bot.world.z).abs());
            if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                best = Some((dist, (spot_x, spot_z)));
            }
        }
        if let Some((_, (spot_x, spot_z))) = best {
            return Some((tier.clone(), spot_x, spot_z));
        }
    }
    None
}

/// Live loc id at a tile (live stream first, static template as fallback).
fn live_loc_id(bot: &Bot, tree_x: i32, tree_z: i32) -> Result<u16, ScriptError> {
    if let Some((loc, _, _)) = bot.world.loc_at(tree_x, tree_z, bot.world.level) {
        return Ok(loc);
    }
    Err(ScriptError::Usage(format!("no loc at {tree_x},{tree_z} from {:?}", bot.position())))
}

/// True only when the server confirms a stump here. Unknown means keep
/// waiting, never assume depletion.
fn is_stump(bot: &Bot, tree_x: i32, tree_z: i32) -> bool {
    match bot.world.locs.get(&(tree_x, tree_z, bot.world.level)) {
        Some(Some((loc, _, _))) => knowledge::loc_by_id(usize::from(*loc))
            .is_some_and(|entry| entry.name.unwrap_or_default().contains("stump")),
        _ => false,
    }
}

fn tree_still_there(bot: &Bot, spot_x: i32, spot_z: i32, tier_names: &[String]) -> bool {
    if bot.world.loc_removed(spot_x, spot_z, bot.world.level) {
        return false;
    }
    let Ok(loc) = live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => entry.name.is_some_and(|name| tier_names.iter().any(|tree| *tree == name.to_lowercase())),
        None => true,
    }
}

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
            println!("ok axe reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Withdraw a banked axe (or its handle+head) and reattach it.
/// Cheaper than rebuying. Reopens the bank: call after a bank trip.
fn restore_from_bank(bot: &mut Bot, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_axe(bot).is_some() {
        return Ok(true);
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
            println!("ok withdrew banked axe");
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
        println!("ok restored axe from banked pieces");
        return Ok(true);
    }
    Ok(false)
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

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range).
fn work_spot(
    bot: &mut Bot,
    tree_x: i32,
    tree_z: i32,
    log_ids: &[u16],
    had_logs: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 4
        && !walk_checked(bot, tree_x, tree_z, 6000).unwrap_or(false)
    {
        return Ok((WorkOutcome::Unreachable, 0));
    }
    if (bot.world.x - tree_x).abs() + (bot.world.z - tree_z).abs() > 1 {
        let mut neighbors: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (tree_x + dx, tree_z + dz)))
            .filter(|tile| *tile != (tree_x, tree_z))
            .collect();
        neighbors.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        let mut stepped = false;
        for (next_x, next_z) in neighbors {
            if bot.walk_to(next_x, next_z, bot.world.level, 100).is_ok() {
                stepped = true;
                break;
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
            println!("click tree at ({tree_x},{tree_z})");
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
        println!("ok replacement axe bought");
        return Ok(true);
    }
    let keep = vec![coins];
    if bank_trip(bot, bank, &keep, 16u32.saturating_sub(pack_total(bot, coins)))? == 0 {
        return Ok(false);
    }
    if best_axe(bot).is_some() {
        println!("ok replacement axe restored");
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
) -> Result<(bool, i64), ScriptError> {
    let log_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut banned: HashMap<(i32, i32), u64> = HashMap::new();
    let idle_since = bot.world.ticks;
    let mut last_hitpoints = hitpoints(bot);
    let mut flees = 0u32;
    let mut spot: Option<(knowledge::trees::TreeTier, i32, i32)> = None;
    let mut had_logs = 0u32;
    loop {
        check_events(bot)?;
        if free_slots(bot) <= usize::from(pack_total(bot, coins) > 0) {
            return Ok((true, level));
        }
        if best_axe(bot).is_none() && !recover_axe(bot, coins, bank)? {
            println!("axe lost, banking load");
            return Ok((true, level));
        }
        level = learn_level(bot, WC_STAT, level);
        if let (Some((tier, _, _)), Some(best)) = (&spot, pick_tier(tiers, level)) {
            if tier.level < best.level {
                println!("level up, switching tier");
                spot = None;
            }
        }
        if hitpoints(bot) == 0 {
            println!("died, respawning");
            bot.intend("died, respawning");
            let target = bot.world.ticks + 40;
            while bot.world.ticks < target && hitpoints(bot) == 0 {
                bot.pump(5)?;
            }
            last_hitpoints = hitpoints(bot);
            spot = None;
            continue;
        }
        if hitpoints(bot) < last_hitpoints {
            flees += 1;
            println!("hurt, fleeing");
            bot.intend("fleeing a hunter");
            if let Some((_, spot_x, spot_z)) = &spot {
                banned.insert((*spot_x, *spot_z), bot.world.ticks + FLEE_BAN_TICKS);
            }
            if flees > MAX_FLEES_PER_LOAD {
                println!("too many attacks, aborting load");
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
                println!("no tree spot, waiting for respawns");
                bot.intend("waiting for a safe tree spot");
                wait_ticks(bot, 15)?;
            }
            let Some((tier, spot_x, spot_z)) = picked else {
                println!("no tree spot left");
                return Ok((false, level));
            };
            had_logs = log_ids.iter().map(|id| pack_total(bot, *id)).sum();
            println!("chopping {} at ({spot_x},{spot_z})", tier.product);
            bot.intend(&format!("chopping {} at ({spot_x},{spot_z})", tier.product));
            spot = Some((tier, spot_x, spot_z));
        } else if let Some((tier, spot_x, spot_z)) = &spot {
            if !tree_still_there(bot, *spot_x, *spot_z, &tier.trees) {
                println!("tree gone at ({spot_x},{spot_z})");
                banned.insert((*spot_x, *spot_z), now + GONE_BAN_TICKS);
                spot = None;
                continue;
            }
        }
        let Some((_, spot_x, spot_z)) = spot.clone() else {
            continue;
        };
        let (result, gained) = work_spot(bot, spot_x, spot_z, &log_ids, had_logs)?;
        had_logs += gained;
        match result {
            WorkOutcome::Gone => {
                println!("tree quiet, next one");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::NoTool => spot = None,
            WorkOutcome::Unreachable | WorkOutcome::LevelLocked => {
                banned.insert((spot_x, spot_z), now + 500);
                spot = None;
            }
            WorkOutcome::Threat => {
                println!("hunter close, re-picking a spot");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::Blocked => {
                println!("unknown dialog blocking chop, aborting load");
                return Ok((false, level));
            }
            WorkOutcome::Full | WorkOutcome::Moved | WorkOutcome::NoBait => {
                spot = None;
            }
        }
        if bot.world.ticks - idle_since > STALL_TICKS {
            println!("stall: no logs for {STALL_TICKS} ticks");
            return Ok((false, level));
        }
    }
}

/// Cut banked loads in the open session (0 = forever).
pub fn run_woodcut(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        println!("toll gates allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = knowledge::trees::all_tiers();
    let mut level = learn_level(bot, WC_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    println!("bank at ({},{})", bank.x, bank.z);
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut bank_coins = 0u32;
    if best_axe(bot).is_none() || pack_ids(bot).iter().any(|id| !known_axe_ids(bot).contains(id) && !log_ids.contains(id) && *id != coins) {
        if reattach(bot)? {
            println!("ok reattached axe from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = log_ids.iter().copied().collect();
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if best_axe(bot).is_none() && restore_from_bank(bot, &bank)? {
            println!("ok axe restored from bank");
        }
        if best_axe(bot).is_none() {
            // Toll crossing costs 10 coins each way: budget it only when
            // the bank sits across the fence, then pay the guard.
            let need_cross = crate::skill_common::toll_gate_x(bot)
                .is_some_and(|line| (bank.x >= line as i64) != (bot.world.x >= line));
            let target = 16u32 + if need_cross { 10 } else { 0 };
            bank_coins = bank_trip(bot, &bank, &keep, u32::MAX / 2)?;
            let have = pack_total(bot, coins);
            if have < target {
                // No wood in the pack: pull the banked logs back out and
                // sell those (an axe is bought with wood, not with nothing).
                let log_list: Vec<u16> = log_ids.iter().copied().collect();
                let carried: u32 = log_list.iter().map(|id| pack_total(bot, *id)).sum();
                if carried == 0 {
                    let pulled = crate::skill_common::bank_withdraw_items(bot, &bank, &log_list, 10)?;
                    if pulled > 0 {
                        println!("withdrew {pulled} logs from the bank to fund the axe");
                    }
                }
                sell_for_target(bot, &log_list, coins, target - pack_total(bot, coins))?;
            }
            if need_cross && !crate::skill_common::cross_toll_gate(bot)? {
                return Err(ScriptError::Usage(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            if pack_total(bot, coins) >= 16 && buy_stock(bot, "bronze_axe", 1)? > 0 {
                println!("ok bootstrapped bronze axe");
                bank = nearest_bank(bot)?;
            } else {
                return Err(ScriptError::Usage(format!(
                    "cannot bootstrap: no axe, {} coins",
                    pack_total(bot, coins)
                )));
            }
        }
    }
    let mut done = 0u32;
    let mut toolless = 0u32;
    while loads == 0 || done < loads {
        check_events(bot)?;
        let (ok, new_level) = chop_load(bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("chop failed at {:?}", bot.position())));
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
        }
        let keep: Vec<u16> = known_axe_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, cheapest as u32)?;
        for cand in cands {
            if buy_stock(bot, &cand, 1)? > 0 {
                println!("ok upgraded to {cand}");
                break;
            }
        }
        toolless = if best_axe(bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(ScriptError::Usage(format!("no axe and cannot rebuy, aborting at {:?}", bot.position())));
        }
        done += 1;
        println!("ok load {done} banked (wc={level})");
    }
    Ok(0)
}
