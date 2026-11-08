//! Miner: best F2P rock for the level, banks full loads, sells ore to fund
//! pickaxe upgrades, reattaches lost heads instead of rebuying.
//! (Gather loop, bank trip, sell, upgrade.)

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

const MINING_STAT: u8 = 14;
const NEAR_BANK_TILES: i64 = 150;
const ROUND_IDLE_TICKS: u64 = 12;
const HANDLE_NAME: &str = "macro_pickaxehandle";

/// Best pickaxe first.
pub const PICK_ORDER: [&str; 6] =
    ["rune_pickaxe", "adamant_pickaxe", "mithril_pickaxe", "steel_pickaxe", "iron_pickaxe", "bronze_pickaxe"];
/// Wield/mining level reqs (content pickaxes config; the obj dump carries
/// no params, so the six values live here).
pub const PICK_REQS: [(&str, i64); 6] =
    [("bronze_pickaxe", 0), ("iron_pickaxe", 0), ("steel_pickaxe", 6), ("mithril_pickaxe", 21), ("adamant_pickaxe", 31), ("rune_pickaxe", 41)];

fn usable_tiers() -> Vec<knowledge::rocks::RockTier> {
    knowledge::rocks::all_rock_tiers()
        .into_iter()
        .filter(|tier| !tier.product.is_empty() && tier.product_members == Some(false) && tier.product != "blankrune")
        .collect()
}

fn tier_price(tier: &knowledge::rocks::RockTier) -> i64 {
    obj_price(&tier.product)
}

fn pick_tier(tiers: &[knowledge::rocks::RockTier], level: i64) -> Option<knowledge::rocks::RockTier> {
    let mut best: Option<&knowledge::rocks::RockTier> = None;
    for tier in tiers {
        if tier.level <= level && best.as_ref().is_none_or(|known| (tier.level, tier_price(tier)) > (known.level, tier_price(known))) {
            best = Some(tier);
        }
    }
    best.cloned()
}

/// Rock spawns for the tier's loc names within range of the bank.
fn tier_spawns(tier: &knowledge::rocks::RockTier, bank: &knowledge::banks::Bank) -> Vec<(i32, i32)> {
    let mut found = Vec::new();
    for entry in knowledge::all_locs() {
        let name = entry.name.clone().unwrap_or_default().to_lowercase();
        if tier.rocks.iter().any(|rock| *rock == name) {
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

fn pick_rock_spot(
    bot: &Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    banned: &HashSet<(i32, i32)>,
) -> Option<(knowledge::rocks::RockTier, i32, i32)> {
    let mut affordable: Vec<&knowledge::rocks::RockTier> =
        tiers.iter().filter(|tier| tier.level <= level).collect();
    affordable.sort_by_key(|tier| (-tier.level, -tier_price(tier)));
    for tier in affordable {
        let mut best: Option<(i64, (i32, i32))> = None;
        for (spot_x, spot_z) in tier_spawns(tier, bank) {
            if banned.contains(&(spot_x, spot_z)) {
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
fn live_loc_id(bot: &Bot, rock_x: i32, rock_z: i32) -> Result<u16, ScriptError> {
    if let Some((loc, _, _)) = bot.world.loc_at(rock_x, rock_z, bot.world.level) {
        return Ok(loc);
    }
    Err(ScriptError::Usage(format!("no loc at {rock_x},{rock_z} from {:?}", bot.position())))
}

/// True unless the server confirmed a depleted rock here (unknown = assume
/// standing, never assume depletion).
fn rock_still_there(bot: &Bot, spot_x: i32, spot_z: i32, tier: &knowledge::rocks::RockTier) -> bool {
    let Ok(loc) = live_loc_id(bot, spot_x, spot_z) else {
        return true;
    };
    match knowledge::loc_by_id(usize::from(loc)) {
        Some(entry) => {
            let debug = entry.name.unwrap_or_default().to_lowercase();
            tier.rocks.iter().any(|rock| *rock == debug)
        }
        None => true,
    }
}

fn best_pickaxe(bot: &Bot) -> Option<String> {
    let held = pack_ids(bot);
    for name in PICK_ORDER {
        if let Ok(id) = obj_id(bot, name) {
            if held.contains(&id) {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn known_pick_ids(bot: &Bot) -> HashSet<u16> {
    PICK_ORDER.into_iter().filter_map(|name| obj_id(bot, name).ok()).collect()
}

/// Reattach a handle + head carried (or on the ground) instead of rebuying.
fn reattach(bot: &mut Bot) -> Result<bool, ScriptError> {
    let Ok(handle) = obj_id(bot, HANDLE_NAME) else {
        return Ok(false);
    };
    if pack_total(bot, handle) == 0 {
        return Ok(false);
    }
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with("_pickaxehead"))
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
        if best_pickaxe(bot).is_some() {
            bot.log("ok pickaxe reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

fn cheapest_upgrade(bot: &Bot, level: i64) -> (Vec<String>, i64) {
    let current_rank = best_pickaxe(bot).and_then(|tool| PICK_ORDER.iter().position(|name| *name == tool)).unwrap_or(PICK_ORDER.len());
    let cands: Vec<String> = PICK_ORDER[..current_rank]
        .iter()
        .filter(|name| PICK_REQS.iter().find(|(tool, _)| tool == *name).is_some_and(|(_, req)| level >= *req))
        .filter(|name| crate::skill_common::shop_stock_keeper(bot, name).is_some())
        .map(|name| name.to_string())
        .collect();
    // shortcut: only surface-reachable shops are reachable with the
    // single-level router (Nurmof sits down the dwarven-mine stairs), so in
    // practice only the bronze pickaxe is ever in stock; the upgrade loop
    // rebuys better ones as soon as stairs traversal lands.
    let cheapest = cands.iter().map(|name| obj_price(name)).min().unwrap_or(0);
    (cands, cheapest)
}

/// Click once, then only re-click after a quiet round (the server chains
/// attempts itself while in range).
fn work_spot(
    bot: &mut Bot,
    rock_x: i32,
    rock_z: i32,
    ore_ids: &[u16],
    had_ore: u32,
) -> Result<(WorkOutcome, u32), ScriptError> {
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 4 {
        // Dead socket or operator stop, not an unreachable rock: typed
        // all the way to the session instead of a quiet Unreachable.
        match walk_checked(bot, rock_x, rock_z, 6000) {
            Ok(true) => {}
            Ok(false) => return Ok((WorkOutcome::Unreachable, 0)),
            Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
            Err(_) => return Ok((WorkOutcome::Unreachable, 0)),
        }
    }
    if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 1 {
        let mut neighbors: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (rock_x + dx, rock_z + dz)))
            .filter(|tile| *tile != (rock_x, rock_z))
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
            if (bot.world.x - rock_x).abs() + (bot.world.z - rock_z).abs() > 4 {
                return Ok((WorkOutcome::Unreachable, 0));
            }
        }
        if !stepped {
            return Ok((WorkOutcome::Unreachable, 0));
        }
    }
    let rock_loc = live_loc_id(bot, rock_x, rock_z)?;
    let mut gained = 0u32;
    let mut silences = 0u32;
    let mut need_click = true;
    let mut seen_msgs = bot.world.messages.len();
    let hitpoints_before = hitpoints(bot);
    loop {
        if need_click {
            bot.log(&format!("click rock at ({rock_x},{rock_z})"));
            bot.intend(&format!("mining a rock at ({rock_x},{rock_z})"));
            bot.op_loc(1, rock_x, rock_z, rock_loc)?;
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
                if lowered.contains("mining level of") {
                    return Ok((WorkOutcome::LevelLocked, gained));
                }
                if lowered.contains("do not have a pickaxe") {
                    return Ok((WorkOutcome::NoTool, gained));
                }
                if lowered.contains("no ore currently available") || lowered.contains("no minerals currently available") {
                    return Ok((WorkOutcome::Gone, gained));
                }
            }
            let now_have: u32 = ore_ids.iter().map(|id| pack_total(bot, *id)).sum();
            if now_have > had_ore + gained {
                gained = now_have - had_ore;
                quiet = false;
                break;
            }
        }
        if !quiet {
            continue;
        }
        silences += 1;
        if silences >= 2 {
            return Ok((WorkOutcome::Gone, gained));
        }
        need_click = true;
    }
}

/// Withdraw a banked pickaxe (or its handle+head) and reattach it.
/// Cheaper than rebuying. Reopens the bank: call after a bank trip.
fn restore_from_bank(bot: &mut Bot, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_pickaxe(bot).is_some() {
        return Ok(true);
    }
    let Ok(handle) = obj_id(bot, HANDLE_NAME) else {
        return Ok(false);
    };
    let heads: Vec<u16> = knowledge::all_objs()
        .into_iter()
        .filter(|entry| entry.name.as_deref().unwrap_or("").ends_with("_pickaxehead"))
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
    let picks = known_pick_ids(bot);
    if let Some((slot, (held, _))) = slots.iter().find(|(_, (held, _))| picks.contains(held)) {
        let (slot, held) = (*slot, *held);
        bot.inv_button(1, held, slot, main_com)?;
        bot.pump(10)?;
        bot.close_modal()?;
        bot.pump(5)?;
        if best_pickaxe(bot).is_some() {
            bot.log("ok withdrew banked pickaxe");
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
        bot.log("ok restored pickaxe from banked pieces");
        return Ok(true);
    }
    Ok(false)
}

fn recover_pickaxe(bot: &mut Bot, coins: u16, bank: &knowledge::banks::Bank) -> Result<bool, ScriptError> {
    if best_pickaxe(bot).is_some() {
        return Ok(true);
    }
    if reattach(bot)? {
        return Ok(true);
    }
    let bronze_price = obj_price("bronze_pickaxe") as u32;
    if pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
        bot.log("ok replacement pickaxe bought");
        return Ok(true);
    }
    let Some(home) = nearest_bank(bot).ok() else {
        return Ok(false);
    };
    let keep = vec![coins];
    if bank_trip(bot, &home, &keep, bronze_price.saturating_sub(pack_total(bot, coins)))? == 0 {
        let _ = bank;
        return Ok(false);
    }
    if best_pickaxe(bot).is_none() && restore_from_bank(bot, &home)? {
        bot.log("ok replacement pickaxe restored from bank");
        return Ok(true);
    }
    if best_pickaxe(bot).is_some() {
        bot.log("ok replacement pickaxe restored");
        return Ok(true);
    }
    Ok(pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0)
}

fn mine_load(
    bot: &mut Bot,
    tiers: &[knowledge::rocks::RockTier],
    bank: &mut knowledge::banks::Bank,
    coins: u16,
    mut level: i64,
) -> Result<(bool, i64), ScriptError> {
    let ore_ids: Vec<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut banned: HashMap<(i32, i32), u64> = HashMap::new();
    let mut idle_since = bot.world.ticks;
    let mut last_hitpoints = hitpoints(bot);
    let mut flees = 0u32;
    let mut spot: Option<(knowledge::rocks::RockTier, i32, i32)> = None;
    let mut had_ore = 0u32;
    loop {
        check_events(bot)?;
        if free_slots(bot) <= usize::from(pack_total(bot, coins) > 0) {
            return Ok((true, level));
        }
        if best_pickaxe(bot).is_none() {
            // Recovery walks (shop, bank) are not mining: don't bill
            // them to the no-ore stall budget.
            let spent = bot.world.ticks;
            if !recover_pickaxe(bot, coins, bank)? {
                bot.log("pickaxe lost, banking load");
                return Ok((true, level));
            }
            idle_since += bot.world.ticks - spent;
        }
        level = learn_level(bot, MINING_STAT, level);
        if let (Some((tier, _, _)), Some(best)) = (&spot, pick_tier(tiers, level)) {
            if best.level > tier.level {
                bot.log("level up, switching tier");
                spot = None;
            }
        }
        if hitpoints(bot) == 0 {
            bot.log("died, respawning");
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
                if let Some(found) = pick_rock_spot(bot, tiers, bank, level, &banned.keys().copied().collect()) {
                    picked = Some(found);
                    break;
                }
                bot.log("no rock spot, waiting for respawns");
                bot.intend("waiting for a safe rock spot");
                wait_ticks(bot, 15)?;
            }
            let Some((tier, spot_x, spot_z)) = picked else {
                bot.log("no rock spot left");
                return Ok((false, level));
            };
            had_ore = ore_ids.iter().map(|id| pack_total(bot, *id)).sum();
            bot.log(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
            bot.intend(&format!("mining {} at ({spot_x},{spot_z})", tier.product));
            spot = Some((tier, spot_x, spot_z));
        } else if let Some((tier, spot_x, spot_z)) = &spot {
            if !rock_still_there(bot, *spot_x, *spot_z, tier) {
                bot.log(&format!("rock gone at ({spot_x},{spot_z})"));
                banned.insert((*spot_x, *spot_z), now + GONE_BAN_TICKS);
                spot = None;
                continue;
            }
        }
        let Some((_, spot_x, spot_z)) = spot.clone() else {
            continue;
        };
        let (result, gained) = work_spot(bot, spot_x, spot_z, &ore_ids, had_ore)?;
        had_ore += gained;
        match result {
            WorkOutcome::Gone => {
                bot.log("rock quiet, next one");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::NoTool => spot = None,
            WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot | WorkOutcome::LevelLocked => {
                banned.insert((spot_x, spot_z), now + 500);
                spot = None;
            }
            WorkOutcome::Threat => {
                bot.log("hunter close, re-picking a spot");
                banned.insert((spot_x, spot_z), now + GONE_BAN_TICKS);
                spot = None;
            }
            WorkOutcome::Blocked => {
                bot.log("unknown dialog blocking mine, aborting load");
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
            bot.log(&format!("stall: no ore for {STALL_TICKS} ticks"));
            return Ok((false, level));
        }
    }
}

/// Mine banked loads in the open session (0 = forever).
pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let loads: u32 = args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0);
    if args.iter().any(|arg| arg == "toll") {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = usable_tiers();
    let mut level = learn_level(bot, MINING_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut bank_coins = 0u32;
    if best_pickaxe(bot).is_none() || pack_ids(bot).iter().any(|id| !known_pick_ids(bot).contains(id) && *id != coins) {
        if reattach(bot)? {
            bot.log("ok pickaxe reattached from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = known_pick_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if best_pickaxe(bot).is_none() && restore_from_bank(bot, &bank)? {
            bot.log("ok pickaxe restored from bank");
        }
        if best_pickaxe(bot).is_none() {
            let bronze_price = obj_price("bronze_pickaxe") as u32;
            let need_cross = crate::skill_common::toll_gate_x(bot)
            .is_some_and(|line| (bank.x as i32 >= line) != (bot.world.x >= line));
            let target = bronze_price + if need_cross { 10 } else { 0 };
            if pack_total(bot, coins) < target {
                bank_coins = bank_trip(bot, &bank, &keep, u32::MAX / 2)?;
            }
            if need_cross && pack_total(bot, coins) >= target && !crate::skill_common::cross_toll_gate(bot)? {
                return Err(ScriptError::Usage(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            if pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
                bot.log("ok bootstrapped bronze pickaxe");
            } else {
                return Err(ScriptError::Usage(format!(
                    "cannot bootstrap: no pickaxe, {} coins",
                    pack_total(bot, coins)
                )));
            }
        }
    }
    let mut done = 0u32;
    let mut toolless = 0u32;
    while loads == 0 || done < loads {
        check_events(bot)?;
        let (ok, new_level) = mine_load(bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("mine failed at {:?}", bot.position())));
        }
        let (cands, cheapest) = cheapest_upgrade(bot, level);
        if !cands.is_empty() {
            sell_for_target(bot, &ore_ids_for(bot, &tiers), coins, cheapest as u32)?;
        }
        let keep: Vec<u16> = known_pick_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, cheapest as u32)?;
        for cand in cands {
            if buy_stock(bot, &cand, 1)? > 0 {
                bot.log(&format!("ok upgraded to {cand}"));
                break;
            }
        }
        toolless = if best_pickaxe(bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(ScriptError::Usage(format!("no pickaxe and cannot rebuy, aborting at {:?}", bot.position())));
        }
        done += 1;
        bot.log(&format!("ok load {done} banked (mining={level})"));
    }
    let _ = bank_coins;
    Ok(0)
}

fn ore_ids_for(bot: &Bot, tiers: &[knowledge::rocks::RockTier]) -> Vec<u16> {
    tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect()
}
