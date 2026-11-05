//! Shared core for the four trade-skill scripts: pack helpers, tick-based
//! walking, banking, shop trade, modal handling and random-event checks.
//!
//! Every wait is tick-bounded (`bot.world.ticks`); nothing sleeps. Errors
//! are typed (`ScriptError::Sdk`, never swallowed); unknown ids or missing
//! interfaces fail loudly instead of falling back silently.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::{wait_backpack, wait_hitpoints, wait_ticks};
use lifescape_sdk::{Bot, Error};

use crate::{ScriptError, ScriptResult};

pub const BACKPACK_SIZE: usize = 28;
pub const TREK_LIMIT: i64 = 400;
pub const STALL_TICKS: u64 = 1000;
pub const MAX_FLEES_PER_LOAD: u32 = 2;
pub const FLEE_BAN_TICKS: u64 = 500;
pub const GONE_BAN_TICKS: u64 = 250;
/// Nearby transition scan: pockets (basements, upper floors) only hold
/// their own ladder within a few dozen tiles.
const RIDE_SCAN_TILES: i32 = 400;
/// Ride only when the ladder gets us this much closer to a bank. Small
/// detours through doors are the router's job, not a teleport's.
const RIDE_MIN_GAIN: i64 = 100;
/// Max ladder rides per bank lookup: one pocket exit, rarely two chained.
const MAX_RIDES: u32 = 3;

/// Spot work result, shared by the three gathering scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkOutcome {
    Full,
    Gone,
    Moved,
    NoTool,
    NoBait,
    Threat,
    Blocked,
    LevelLocked,
    Unreachable,
}

/// Resolve an object id through the injected tables. Unknown names fail,
// never guess.
pub fn obj_id(bot: &Bot, name: &str) -> Result<u16, ScriptError> {
    (bot.tables.find_obj_by_name)(name)
        .ok_or_else(|| ScriptError::Usage(format!("unknown obj {name:?} at {:?}", bot.position())))
}

/// Shop price from the dump (0 when the dump carries none).
pub fn obj_price(name: &str) -> i64 {
    knowledge::objs_by_name(name).first().map(|entry| entry.price).unwrap_or(0)
}

/// Display name for logs, empty when the tables do not know the id.
pub fn obj_label(bot: &Bot, obj: u16) -> String {
    (bot.tables.obj_name)(obj).unwrap_or_default()
}

/// (slot, (obj, count)) for every backpack slot.
pub fn pack_slots(bot: &Bot) -> Vec<(u8, (u16, u32))> {
    bot.backpack().1
}

/// Total carried count of one object id.
pub fn pack_total(bot: &Bot, obj: u16) -> u32 {
    pack_slots(bot).iter().filter(|(_, (held, _))| *held == obj).map(|(_, (_, count))| *count).sum()
}

/// First backpack slot holding the object, or a loud error.
pub fn pack_slot(bot: &Bot, obj: u16) -> Result<u8, ScriptError> {
    pack_slots(bot)
        .iter()
        .find(|(_, (held, _))| *held == obj)
        .map(|(slot, _)| *slot)
        .ok_or_else(|| Error::Unreachable(format!("no obj {obj} in pack at {:?}", bot.position())).into())
}

/// Free backpack slots (28-cap, corpses of full loads).
pub fn free_slots(bot: &Bot) -> usize {
    BACKPACK_SIZE.saturating_sub(pack_slots(bot).len())
}

/// Distinct object ids currently carried.
pub fn pack_ids(bot: &Bot) -> HashSet<u16> {
    pack_slots(bot).iter().map(|(_, (held, _))| *held).collect()
}

/// Live level of a stat, or the last known value before it streamed.
pub fn learn_level(bot: &Bot, stat: u8, known: i64) -> i64 {
    bot.world.stats.get(&stat).map(|(level, _)| i64::from(*level)).unwrap_or(known)
}

/// Current hitpoints (10 before the stats stream).
pub fn hitpoints(bot: &Bot) -> u8 {
    bot.world.stats.get(&3).map(|(level, _)| *level).unwrap_or(10)
}

/// Combat level (3 before the stats stream).
pub fn combat_level(bot: &Bot) -> i64 {
    i64::from(bot.combat_level().unwrap_or(3))
}

/// Random-event gate: greets hosts on sight, rubs the lamp, solves what
/// resolves inline and prints the `@@EVENT` payload otherwise, so the
/// orchestrator unloads this script and runs the mapped rescue.
pub fn check_events(bot: &mut Bot) -> ScriptResult {
    if !crate::randomevent_full::check(bot)? {
        return Err(ScriptError::Usage(format!("random event unresolved at {:?}", bot.position())));
    }
    Ok(0)
}

/// Wait until the backpack streams after login (tick-bounded).
pub fn wait_pack_streamed(bot: &mut Bot) -> Result<(), ScriptError> {
    if !wait_backpack(bot, 150)? {
        return Err(ScriptError::Usage(format!("backpack never streamed at {:?}", bot.position())));
    }
    Ok(())
}

/// Dismiss a level-up / dialog modal with Continue. True when no modal
/// remains; congratulation dialogs are the only ones auto-dismissed.
pub fn dismiss_modal(bot: &mut Bot) -> Result<bool, ScriptError> {
    if bot.world.modal.is_none() {
        return Ok(true);
    }
    let congrats = bot.chat_texts().iter().any(|(_, text)| text.to_lowercase().contains("congratulations"));
    if !congrats {
        return Ok(false);
    }
    for _ in 0..4 {
        bot.chat_continue()?;
        bot.pump(5)?;
        if bot.world.modal.is_none() {
            println!("ok dialog dismissed");
            return Ok(true);
        }
    }
    let target = bot.world.ticks + 25;
    while bot.world.ticks < target && bot.world.modal.is_some() {
        bot.pump(5)?;
    }
    Ok(bot.world.modal.is_none())
}

/// Chunked `walk_to` in 40-tile legs with event checks: a hurt walk aborts
/// (Ok(false)) so the caller can flee, a stuck walk errors loudly.
pub fn walk_checked(bot: &mut Bot, dest_x: i32, dest_z: i32, timeout: u64) -> Result<bool, ScriptError> {
    let deadline = bot.world.ticks + timeout;
    let mut hitpoints_before = hitpoints(bot);
    let mut last_greet = bot.world.ticks;
    while (bot.world.x, bot.world.z) != (dest_x, dest_z) {
        if bot.world.ticks > deadline {
            return Err(Error::Timeout(format!(
                "walk to {dest_x},{dest_z} timed out at {:?} ({timeout} ticks)",
                bot.position()
            ))
            .into());
        }
        if bot.world.ticks - last_greet > 50 {
            last_greet = bot.world.ticks;
            check_events(bot)?;
        }
        let step_x = bot.world.x + (dest_x - bot.world.x).clamp(-40, 40);
        let step_z = bot.world.z + (dest_z - bot.world.z).clamp(-40, 40);
        let rest = deadline.saturating_sub(bot.world.ticks).max(500).min(6000);
        match bot.walk_to(step_x, step_z, bot.world.level, rest) {
            Ok(()) => {}
            Err(err) => {
                if (bot.world.x - dest_x).abs() + (bot.world.z - dest_z).abs() <= 2 {
                    return Ok(true);
                }
                return Err(err.into());
            }
        }
        let now_hitpoints = hitpoints(bot);
        if now_hitpoints < hitpoints_before {
            println!("hurt walking, fleeing");
            return Ok(false);
        }
        hitpoints_before = now_hitpoints;
    }
    Ok(true)
}

/// Closest free same-level bank booth by Manhattan distance, or i64::MAX
/// when no booth fits. Pure scan (no walking): the gain test for a ladder
/// ride and the toll-aware pick below share it.
fn closest_bank_dist(here_x: i32, here_z: i32, level: i32) -> i64 {
    let mut best = i64::MAX;
    for bank in knowledge::banks::all_banks() {
        if bank.members || bank.level != i64::from(level) {
            continue;
        }
        let dist = (bank.x - i64::from(here_x)).abs() + (bank.z - i64::from(here_z)).abs();
        if dist < best {
            best = dist;
        }
    }
    best
}

/// A mapped transition worth riding: same level, close enough to walk to,
/// and landing much closer to a bank than we are (basement ladder to the
/// surface, upper floor down to the bankhall...). None when the walk
/// already wins: the router owns small detours, not a teleport.
fn shortcut_out(bot: &Bot, here_best: i64) -> Option<lifescape_sdk::TransitionRow> {
    let mut cands: Vec<(i32, lifescape_sdk::TransitionRow)> = Vec::new();
    for row in &bot.transitions {
        if row.disabled || row.from_tile.0 != bot.world.level {
            continue;
        }
        let walk = (row.from_tile.1 - bot.world.x).abs() + (row.from_tile.2 - bot.world.z).abs();
        if walk > RIDE_SCAN_TILES {
            continue;
        }
        let there_best = closest_bank_dist(row.to_tile.1, row.to_tile.2, row.to_tile.0);
        if there_best == i64::MAX {
            continue;
        }
        if here_best.saturating_sub(i64::from(walk).saturating_add(there_best)) > RIDE_MIN_GAIN {
            cands.push((walk, row.clone()));
        }
    }
    cands.sort_by_key(|(walk, _)| *walk);
    cands.into_iter().map(|(_, row)| row).next()
}

/// Ride one transition row: stand on its tile (footprint included, multi-
/// tile stairs answer a click on one tile only), click the table's op,
/// wait for the position to move. True when we left the tile: same-level
/// rides move x/z, floor rides move level, both count. A failed ride is
/// Ok(false), never a guess elsewhere; only an operator stop propagates.
fn ride_row(bot: &mut Bot, row: &lifescape_sdk::TransitionRow) -> Result<bool, ScriptError> {
    let (level, from_x, from_z) = row.from_tile;
    bot.intend(&format!("riding {} out", row.name));
    let mut tiles = vec![(from_x, from_z)];
    for tile in &row.footprint {
        if *tile != (from_x, from_z) {
            tiles.push(*tile);
        }
    }
    for (click_x, click_z) in tiles {
        if let Err(err) = bot.approach(click_x, click_z, level, 1, 800) {
            if matches!(err, Error::Interrupted(_)) {
                return Err(err.into());
            }
            continue;
        }
        let loc_id = bot
            .world
            .loc_at(click_x, click_z, level)
            .map(|(id, _, _)| id)
            .or_else(|| bot.world.static_locs.get(&(click_x, click_z, level)).map(|entry| entry.0))
            .unwrap_or(row.loc);
        let before = bot.position();
        match bot.op_loc(row.op, click_x, click_z, loc_id) {
            Err(Error::Interrupted(_)) => return Err(Error::Interrupted(format!("ride {} stopped", row.name)).into()),
            Err(_) => continue,
            Ok(_) => {}
        }
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target && bot.position() == before {
            bot.pump(2)?;
        }
        if bot.position() != before {
            println!("rode {} to {:?}", row.name, bot.position());
            return Ok(true);
        }
    }
    println!("ride {} failed from {:?}", row.name, bot.position());
    Ok(false)
}

/// Ride out of a disconnected pocket toward the banks: a mapped ladder
/// or stairs whose far side is much closer to a bank than here (basement
/// to the surface, upper floors down). True when it moved us somewhere
/// with a better bank urge; the caller re-picks from the new position.
pub fn escape_pocket(bot: &mut Bot) -> Result<bool, ScriptError> {
    let here_best = closest_bank_dist(bot.world.x, bot.world.z, bot.world.level);
    let Some(row) = shortcut_out(bot, here_best) else {
        return Ok(false);
    };
    ride_row(bot, &row)
}

/// Nearest free (non-members, same level) bank booth by Manhattan distance.
/// Nearest bankhall, F2P and on this level. Toll-fence aware: banks on
/// the far side of a toll gate are last resort, never the default.
/// Needs the toll gate's fence line, read off the loaded toll doors.
///
/// Transition-aware: Manhattan means nothing across disconnected regions
/// (a basement shares its level with the surface but no walk). When a
/// mapped ladder/stairs nearby leads somewhere much closer to a bank,
/// ride it first, then pick from the new position.
pub fn nearest_bank(bot: &mut Bot) -> Result<knowledge::banks::Bank, ScriptError> {
    for _ in 0..MAX_RIDES {
        if !escape_pocket(bot)? {
            break;
        }
    }
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    bot.ensure_area(here_x, here_z, 150);
    let gate_x = bot
        .toll_gates
        .iter()
        .filter(|(_, _, level)| *level == bot.world.level)
        .min_by_key(|(x, z, _)| (x - here_x).abs() + (z - here_z).abs())
        .map(|(x, _, _)| *x);
    let mut best: Option<(i64, knowledge::banks::Bank)> = None;
    let mut fallback: Option<(i64, knowledge::banks::Bank)> = None;
    for bank in knowledge::banks::all_banks() {
        if bank.members || bank.level != i64::from(bot.world.level) {
            continue;
        }
        let dist = (bank.x - i64::from(here_x)).abs() + (bank.z - i64::from(here_z)).abs();
        let across = gate_x.is_some_and(|line| (bank.x >= line as i64) != (here_x as i64 >= line as i64));
        let slot = if across { &mut fallback } else { &mut best };
        let weaker = match slot {
            Some((known, _)) => dist < *known,
            None => true,
        };
        if weaker {
            *slot = Some((dist, bank));
        }
    }
    best.or(fallback)
        .map(|(_, bank)| bank)
        .ok_or_else(|| ScriptError::Usage(format!("no reachable bank from {:?}", bot.position())))
}

/// X of the nearest toll-gate door, if a fence has been mapped yet.
pub fn toll_gate_x(bot: &mut Bot) -> Option<i32> {
    let (here_x, here_z) = (bot.world.x, bot.world.z);
    bot.ensure_area(here_x, here_z, 150);
    bot.toll_gates
        .iter()
        .filter(|(_, _, level)| *level == bot.world.level)
        .min_by_key(|(x, z, _)| (x - here_x).abs() + (z - here_z).abs())
        .map(|(x, _, _)| *x)
}

/// Cross the toll fence through the guard's dialog (10 coins). The side
/// flip proves it: works eastward and westward, unlike a fixed check.
pub fn cross_toll_gate(bot: &mut Bot) -> Result<bool, ScriptError> {
    let Some(line) = toll_gate_x(bot) else {
        println!("no toll gate found near {:?}", bot.position());
        return Ok(false);
    };
    let start_side = bot.world.x >= line;
    bot.intend(&format!("paying the toll gate at x={line}"));
    let target = bot.world.ticks + 1500;
    while bot.world.ticks < target && (bot.world.x >= line) == start_side {
        let gate = match bot
            .toll_gates
            .iter()
            .filter(|(_, _, level)| *level == bot.world.level)
            .min_by_key(|(x, z, _)| (x - bot.world.x).abs() + (z - bot.world.z).abs())
            .copied()
        {
            Some(gate) => gate,
            None => return Ok(false),
        };
        bot.approach(gate.0, gate.1, gate.2, 1, 300)?;
        if (bot.world.x >= line) != start_side {
            println!("walked through the toll gate at {:?}", bot.position());
            return Ok(true);
        }
        let gate_id = gate_id(bot, gate)?;
        let Some(op) = bot
            .maps
            .as_ref()
            .and_then(|pack| pack.loc_def(gate_id as usize))
            .and_then(|def| def.open_op)
        else {
            return Ok(false);
        };
        bot.op_loc(op, gate.0, gate.1, gate_id)?;
        let talked = bot.world.ticks + 25;
        while bot.world.ticks < talked && bot.world.modal.is_none() {
            bot.pump(5)?;
        }
        if bot.world.modal.is_none() {
            bot.approach(gate.0, gate.1, gate.2, 1, 300)?;
            continue;
        }
        crate::quest_common::drain_dialog(bot, 60)?;
        if bot.world.modal.is_some() {
            let _ = bot.chat_choose("Yes, ok.");
            bot.pump(10)?;
            crate::quest_common::drain_dialog(bot, 30)?;
        }
        let moved = bot.world.ticks + 30;
        while bot.world.ticks < moved && (bot.world.x >= line) == start_side {
            bot.pump(2)?;
        }
        if (bot.world.x >= line) != start_side {
            println!("through the toll gate at {:?}", bot.position());
            return Ok(true);
        }
    }
    println!("toll gate did not move us at {:?}", bot.position());
    Ok((bot.world.x >= line) != start_side)
}

fn gate_id(bot: &Bot, gate: (i32, i32, i32)) -> Result<u16, ScriptError> {
    bot.world
        .loc_at(gate.0, gate.1, gate.2)
        .map(|(id, _, _)| id)
        .or_else(|| bot.world.static_locs.get(&gate).map(|entry| entry.0))
        .ok_or_else(|| ScriptError::Usage(format!("toll gate gone at {gate:?}")))
}

/// Live bankbooth loc nearest to the booth spawn (loc id comes from the
/// live stream, never assumed from the dump).
pub fn live_bank_booth(bot: &Bot, bank_x: i32, bank_z: i32) -> Option<(i32, i32, u16)> {
    let mut best: Option<(i64, (i32, i32, u16))> = None;
    let mut near: Option<(i64, String, u16)> = None;
    let mut consider = |x: i32, z: i32, level: i32, loc: u16| {
        if level != bot.world.level {
            return;
        }
        let dist = i64::from((x - bank_x).abs() + (z - bank_z).abs());
        let label = (bot.tables.loc_name)(loc).unwrap_or_default().to_lowercase();
        if !label.contains("bankbooth") && !label.contains("bank booth") {
            if near.as_ref().is_none_or(|(known, _, _)| dist < *known) {
                near = Some((dist, label, loc));
            }
            return;
        }
        if best.as_ref().is_none_or(|(known, _)| dist < *known) {
            best = Some((dist, (x, z, loc)));
        }
    };
    for ((x, z, level), slot) in &bot.world.locs {
        if let Some((loc, _, _)) = slot {
            consider(*x, *z, *level, *loc);
        }
    }
    for ((x, z, level), (loc, _, _)) in &bot.world.static_locs {
        consider(*x, *z, *level, *loc);
    }
    if best.is_none() {
        println!("bank search: live={} static={} near={:?}", bot.world.locs.len(), bot.world.static_locs.len(), near);
    }
    best.map(|(_, found)| found)
}

/// Withdraw up to `count` items total, across `ids`, from the bank at
/// `bank`. Used to pull banked produce back out for selling (the axe
/// bootstrap sells wood it banked on earlier runs). Returns how many came
/// out of the bank. Bounded in ticks, loud when the bank will not open.
pub fn bank_withdraw_items(
    bot: &mut Bot,
    bank: &knowledge::banks::Bank,
    ids: &[u16],
    count: u32,
) -> Result<u32, ScriptError> {
    if count == 0 || ids.is_empty() {
        return Ok(0);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
        println!("bank withdraw: no live bankbooth near ({},{})", bank.x, bank.z);
        return Ok(0);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        println!("bank withdraw: bank did not open at {:?}", bot.position());
        return Ok(0);
    }
    let Some(main_com) = (bot.tables.com_id)("bank_main:inv") else {
        bot.close_modal()?;
        return Ok(0);
    };
    let before: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    let mut remaining = count;
    while remaining > 0 {
        let slots = bot.world.inv.get(&main_com).cloned().unwrap_or_default();
        let Some((slot, held)) = slots
            .iter()
            .filter(|(_, (held, n))| ids.contains(held) && *n > 0)
            .map(|(slot, (held, _))| (*slot, *held))
            .next()
        else {
            break;
        };
        // Bank buttons: 1 = one, 2 = five (all for coins, handled elsewhere).
        let (op, step) = if remaining >= 5 { (2u8, 5u32) } else { (1u8, 1u32) };
        bot.inv_button(op, held, slot, main_com)?;
        bot.pump(5)?;
        remaining = remaining.saturating_sub(step);
    }
    bot.close_modal()?;
    bot.pump(5)?;
    let after: u32 = ids.iter().map(|id| pack_total(bot, *id)).sum();
    Ok(after.saturating_sub(before))
}

/// Bank a load, re-discovering the bank when the trip leaves produce
/// behind (teleport mid-run, pocket start). One retry from the fresh
/// bank, then loud: the load is stuck, not silently unbanked.
pub fn bank_trip_resilient(
    bot: &mut Bot,
    bank: &mut knowledge::banks::Bank,
    keep: &[u16],
    withdraw_coins: u32,
    product_ids: &[u16],
) -> Result<u32, ScriptError> {
    let mut bank_coins = bank_trip(bot, bank, keep, withdraw_coins)?;
    if product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
        println!("bank unreachable from {:?}, re-discovering", bot.position());
        *bank = nearest_bank(bot)?;
        bank_coins = bank_trip(bot, bank, keep, withdraw_coins)?;
        if product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
            return Err(ScriptError::Usage(format!("bank unreachable, load stuck at {:?}", bot.position())));
        }
    }
    Ok(bank_coins)
}

/// Walk to a bank, open the booth, deposit everything but `keep`, withdraw
/// `withdraw_coins`. Returns the banked coin total for upgrade budgeting.
pub fn bank_trip(
    bot: &mut Bot,
    bank: &knowledge::banks::Bank,
    keep: &[u16],
    withdraw_coins: u32,
) -> Result<u32, ScriptError> {
    let side_com = (bot.tables.com_id)("bank_side:inv")
        .ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    println!("bank trip to ({},{}) from {:?}", bank.x, bank.z, bot.position());
    bot.intend(&format!("walking to the bank at ({},{})", bank.x, bank.z));
    // The genie lamp is unbankable: depositing it always fails, so keep it.
    // A later randomevent check rubs it for XP instead.
    let mut keep: Vec<u16> = keep.to_vec();
    if let Some(lamp) = (bot.tables.find_obj_by_name)("macro_genilamp") {
        if !keep.contains(&lamp) {
            keep.push(lamp);
        }
    }
    let bank_x = bank.x as i32;
    let bank_z = bank.z as i32;
    let bank_level = bank.level as i32;
    let mut reached = false;
    bot.ensure_area(bank_x, bank_z, 32);
    for _ in 0..3 {
        match bot.approach(bank_x, bank_z, bank_level, 6, 15000) {
            Ok(()) => {
                reached = true;
                break;
            }
            Err(lifescape_sdk::Error::Interrupted(_)) => {
                return Err(ScriptError::Sdk(lifescape_sdk::Error::Interrupted(format!(
                    "bank trip stopped at {:?}",
                    bot.position()
                ))));
            }
            Err(other) => {
                println!("bank trip: approach failed ({other}), waiting a tick");
                wait_hitpoints(bot, hitpoints(bot).saturating_add(1), 200)?;
            }
        }
    }
    if !reached {
        println!("bank trip: could not reach ({bank_x},{bank_z})");
        return Ok(0);
    }
    let Some((booth_x, booth_z, booth_loc)) = live_bank_booth(bot, bank_x, bank_z) else {
        println!("bank trip: no live bankbooth near ({bank_x},{bank_z})");
        return Ok(0);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        println!("bank trip: bank did not open at {:?}", bot.position());
        return Ok(0);
    }
    let deposited = bot.bank_deposit_all(side_com, &keep, 25)?;
    if !deposited {
        println!("bank trip: deposit left leftovers at {:?}", bot.position());
    }
    if withdraw_coins > 0 {
        if let Ok(coins) = obj_id(bot, "coins") {
            let _ = bot.bank_withdraw(coins, withdraw_coins, 50);
            bot.pump(5)?;
        }
    }
    let coins_now = obj_id(bot, "coins").ok();
    let bank_coins = coins_now.and_then(|coins| bot.bank_items().get(&coins).copied()).unwrap_or(0);
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(bank_coins)
}

/// Nearest general-store keeper spawn on this level within trek range.
pub fn general_store(bot: &Bot) -> Option<(String, (i32, i32, i32))> {
    let mut best: Option<(i64, (String, (i32, i32, i32)))> = None;
    for shop in knowledge::shops::all_shops() {
        if !shop.name.to_lowercase().contains("general") {
            continue;
        }
        for keeper in &shop.keepers {
            for entry in knowledge::npcs_by_name(&keeper.npc) {
                for spawn in &entry.spawns {
                    if spawn.level != i64::from(bot.world.level) {
                        continue;
                    }
                    let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                    if dist > TREK_LIMIT {
                        continue;
                    }
                    let tile = (spawn.level as i32, spawn.x as i32, spawn.z as i32);
                    if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                        best = Some((dist, (keeper.npc.clone(), tile)));
                    }
                }
            }
        }
    }
    best.map(|(_, found)| found)
}

/// Nearest shop stocking `obj_name` (stock count > 0) with its keeper spawn.
// shortcut: keeper NPC op for Trade is assumed to be op 2 (talk/trade op
// order is pack data, not in the gamedata dump); fails loudly otherwise.
pub fn shop_stock_keeper(bot: &Bot, obj_name: &str) -> Option<(String, (i32, i32, i32))> {
    let mut best: Option<(i64, (String, (i32, i32, i32)))> = None;
    for shop in knowledge::shops::all_shops() {
        if !shop.stocks.iter().any(|stock| stock.obj == obj_name && stock.count > 0) {
            continue;
        }
        for keeper in &shop.keepers {
            for entry in knowledge::npcs_by_name(&keeper.npc) {
                for spawn in &entry.spawns {
                    if spawn.level != i64::from(bot.world.level) {
                        continue;
                    }
                    let dist = (spawn.x - i64::from(bot.world.x)).abs() + (spawn.z - i64::from(bot.world.z)).abs();
                    if dist > TREK_LIMIT {
                        continue;
                    }
                    if best.as_ref().is_none_or(|(known, _)| dist < *known) {
                        best = Some((dist, (keeper.npc.clone(), (spawn.level as i32, spawn.x as i32, spawn.z as i32))));
                    }
                }
            }
        }
    }
    best.map(|(_, found)| found)
}

/// 1-based op number for a named NPC op, from the dump (Talk-to is 1).
/// None when the NPC type or the op is unknown: callers fail loudly
/// instead of clicking the wrong verb.
pub fn npc_op(npc_type: u16, op_name: &str) -> Option<u8> {
    lifescape_gamedata::npc_by_id(npc_type as usize)
        .and_then(|entry| entry.ops.iter().position(|name| name.as_deref().is_some_and(|label| label.eq_ignore_ascii_case(op_name))))
        .map(|index| index as u8 + 1)
}

/// Walk to a keeper, open trade, wait for the shop stock com. The keeper
/// name matches live NPCs by substring (same as `Bot::find_npc`).
/// Tries every tile around the keeper like the reference flow does: counters
/// block the click from most sides, so one angle is rarely enough.
pub fn trade_to_keeper(bot: &mut Bot, keeper: &str, spawn: (i32, i32, i32)) -> Result<bool, ScriptError> {
    if !walk_checked(bot, spawn.1, spawn.2, 15000).unwrap_or(false) {
        return Ok(false);
    };
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let deadline = bot.world.ticks + 400;
    for _round in 0..2 {
        if bot.world.ticks >= deadline {
            break;
        }
        let Some(slot) = bot.find_npc(keeper) else {
            bot.pump(5)?;
            continue;
        };
        let npc = match bot.world.npcs.get(&slot) {
            Some(npc) => (npc.x, npc.z, npc.npc_type),
            None => {
                bot.pump(5)?;
                continue;
            }
        };
        let Some(op) = npc_op(npc.2, "trade") else {
            println!("keeper {keeper} has no Trade op at {:?}", bot.position());
            return Ok(false);
        };
        let mut tiles: Vec<(i32, i32)> = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (npc.0 + dx, npc.1 + dz)))
            .filter(|tile| *tile != (npc.0, npc.1))
            .collect();
        tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if bot.world.ticks >= deadline {
                break;
            }
            if bot.walk_to(tile_x, tile_z, bot.world.level, 750).is_err() {
                continue;
            }
            if bot.op_npc(op, slot).is_err() {
                continue;
            }
            let end = bot.world.ticks + 400;
            while bot.world.modal.is_none() && bot.world.ticks < end {
                bot.pump(5)?;
            }
            if bot.world.inv.contains_key(&main_com) {
                return Ok(true);
            }
            if bot.world.modal.is_some() {
                bot.close_modal()?;
                bot.pump(5)?;
            }
        }
    }
    println!("trade with {keeper} failed at {:?}", bot.position());
    Ok(false)
}

/// Sell carried products until `target_coins` are raised (shortfall only,
// never stockpiles sale coins). Returns coins gained.
pub fn sell_for_target(
    bot: &mut Bot,
    product_ids: &[u16],
    coins: u16,
    target_coins: u32,
) -> Result<u32, ScriptError> {
    if target_coins == 0 || !product_ids.iter().any(|id| pack_total(bot, *id) > 0) {
        return Ok(0);
    }
    let side_com = (bot.tables.com_id)("shop_template_side:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template_side:inv missing from component table".to_string()))?;
    let Some((keeper, spawn)) = general_store(bot) else {
        println!("no general store found");
        return Ok(0);
    };
    if !trade_to_keeper(bot, &keeper, spawn)? {
        println!("general store trade failed at {:?}", bot.position());
        return Ok(0);
    }
    let coins_before = pack_total(bot, coins);
    let Some(first) = product_ids.iter().find(|id| pack_total(bot, **id) > 0).copied() else {
        bot.close_modal()?;
        return Ok(0);
    };
    bot.inv_button(2, first, pack_slot(bot, first)?, side_com)?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && pack_total(bot, coins) <= coins_before {
        bot.pump(5)?;
    }
    let unit = pack_total(bot, coins).saturating_sub(coins_before);
    if unit == 0 {
        bot.close_modal()?;
        bot.pump(5)?;
        return Ok(0);
    }
    while pack_total(bot, coins) < coins_before + target_coins {
        let Some(next) = product_ids.iter().find(|id| pack_total(bot, **id) > 0).copied() else {
            break;
        };
        let need = coins_before + target_coins - pack_total(bot, coins);
        let amount = pack_total(bot, next).min(((need + unit - 1) / unit).max(1));
        let Some((op, _)) = sell_batches(amount).first().copied() else {
            break;
        };
        let Ok(slot) = pack_slot(bot, next) else {
            break;
        };
        bot.inv_button(op, next, slot, side_com)?;
        bot.pump(5)?;
    }
    let gained = pack_total(bot, coins) - coins_before;
    println!("ok sold products for {gained} coins");
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(gained)
}

/// Buy `count` of an object from its nearest stocking shop. Returns bought.
pub fn buy_stock(bot: &mut Bot, obj_name: &str, count: u32) -> Result<u32, ScriptError> {
    if count == 0 {
        return Ok(0);
    }
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let Some((keeper, spawn)) = shop_stock_keeper(bot, obj_name) else {
        println!("no shop stocks {obj_name} nearby");
        return Ok(0);
    };
    if !trade_to_keeper(bot, &keeper, spawn)? {
        println!("trade with {keeper} failed");
        return Ok(0);
    }
    let wanted = obj_id(bot, obj_name)?;
    let before = pack_total(bot, wanted);
    let target = bot.world.ticks + 50;
    let mut slot_found = None;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if let Some(slots) = bot.world.inv.get(&main_com) {
            slot_found = slots.iter().find(|(_, (held, _))| *held == wanted).map(|(slot, _)| *slot);
            if slot_found.is_some() {
                break;
            }
        }
    }
    let Some(shop_slot) = slot_found else {
        println!("no {obj_name} in shop stock");
        bot.close_modal()?;
        bot.pump(5)?;
        return Ok(0);
    };
    for (op, _) in sell_batches(count) {
        bot.inv_button(op, wanted, shop_slot, main_com)?;
        bot.pump(5)?;
    }
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && pack_total(bot, wanted) <= before {
        bot.pump(5)?;
    }
    let gained = pack_total(bot, wanted) - before;
    bot.close_modal()?;
    bot.pump(5)?;
    Ok(gained)
}

/// Sell-button batches for an amount: 10s, then 5s, then 1s.
pub fn sell_batches(mut amount: u32) -> Vec<(u8, u32)> {
    let mut batches = Vec::new();
    for (op, size) in [(4u8, 10u32), (3, 5), (2, 1)] {
        while amount >= size {
            batches.push((op, size));
            amount -= size;
        }
    }
    batches
}

/// Bury every bone in the pack (frees slots, trains prayer).
pub fn bury_bones(bot: &mut Bot) -> Result<(), ScriptError> {
    loop {
        let found = pack_slots(bot).into_iter().find(|(_, (held, _))| {
            obj_label(bot, *held).to_lowercase().contains("bones")
        });
        let Some((slot, (held, _))) = found else {
            return Ok(());
        };
        let Some(pack_com) = bot.backpack().0 else {
            return Err(Error::Unreachable("backpack not streamed".to_string()).into());
        };
        bot.op_held(1, held, slot, pack_com)?;
        bot.pump(2)?;
    }
}

/// Pump until hitpoints reach `target` (heal wait after a flee).
pub fn wait_healed(bot: &mut Bot, timeout: u64) -> Result<(), ScriptError> {
    wait_hitpoints(bot, hitpoints(bot).saturating_add(1), timeout)?;
    Ok(())
}

/// Settle-wait until the carried total reaches `minimum` (server inventory
/// updates arrive ticks after the click).
pub fn wait_pack_count(bot: &mut Bot, obj: u16, minimum: u32, timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if pack_total(bot, obj) >= minimum {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Wait until the backpack holds one more free-slot tick cycle of calm.
pub fn settle(bot: &mut Bot, ticks: u64) -> Result<(), ScriptError> {
    wait_ticks(bot, ticks)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{closest_bank_dist, live_bank_booth, shortcut_out};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    #[test]
    fn booth_found_in_static_locs() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        assert!(live_bank_booth(&bot, 3015, 3354).is_none(), "no booth without scenery");
        bot.world.static_locs.insert((3015, 3354, 0), (2213, 0, 0));
        let found = live_bank_booth(&bot, 3015, 3354).expect("Falador booth resolves");
        assert_eq!(found, (3015, 3354, 2213));
        bot.world.static_locs.insert((3200, 3200, 0), (2213, 0, 0));
        let near = live_bank_booth(&bot, 3015, 3354).expect("nearest of two booths");
        assert_eq!(near, (3015, 3354, 2213));
    }

    fn ladder_row() -> lifescape_sdk::TransitionRow {
        lifescape_sdk::TransitionRow {
            name: "wizards_tower_ladder_359".to_string(),
            from_tile: (0, 3103, 9576),
            to_tile: (0, 3105, 3162),
            loc: 2148,
            op: 1,
            footprint: vec![(3103, 9576)],
            disabled: false,
        }
    }

    #[test]
    fn basement_offers_ladder_surface_does_not() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.transitions.push(ladder_row());
        // Pocket start: nearest booth is Edgeville by bogus Manhattan, but
        // the ladder lands next to Draynor: ride it.
        bot.world.x = 3098;
        bot.world.z = 9567;
        bot.world.level = 0;
        let here_best = closest_bank_dist(3098, 9567, 0);
        assert!(here_best > 6000, "basement Manhattan means nothing ({here_best})");
        let row = shortcut_out(&bot, here_best).expect("ladder out of the basement");
        assert_eq!(row.name, "wizards_tower_ladder_359");
        // Surface start: Draynor wins outright, the ladder back down helps
        // nobody.
        bot.world.x = 3105;
        bot.world.z = 3162;
        let surface_best = closest_bank_dist(3105, 3162, 0);
        assert!(surface_best < 100, "Draynor is next door ({surface_best})");
        assert!(shortcut_out(&bot, surface_best).is_none(), "no ride from the surface");
    }
}
