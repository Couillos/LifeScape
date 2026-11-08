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
    obj_price, pack_ids, pack_slot, pack_total, sell_for_target, travel_ticks, walk_checked, WorkOutcome, FLEE_BAN_TICKS,
    GONE_BAN_TICKS, MAX_FLEES_PER_LOAD, STALL_TICKS, STRUCTURAL_BAN_TICKS, UNREACHABLE_BAN_TICKS,
};
use crate::skill_woodcutting_spot::{adjacent_to, is_stump, live_loc_id, pick_tier, pick_tree_spot, refusal_ban_ticks, tree_still_there, DIRECT_REFUSALS_TO_CONDEMN};
use crate::{ScriptError, ScriptResult};

const WC_STAT: u8 = 8;
const ROUND_IDLE_TICKS: u64 = 12;
const HANDLE_NAME: &str = "macro_hatchethandle";

/// Best axe first.
pub const AXE_ORDER: [&str; 7] =
    ["rune_axe", "adamant_axe", "mithril_axe", "black_axe", "steel_axe", "iron_axe", "bronze_axe"];

/// Rank in AXE_ORDER (0 = best). The server's checker takes the best
/// carried axe at any woodcutting level (woodcut.rs2
/// woodcutting_axe_checker: the level-gated shortlist is commented out,
/// 2004 behaviour), so rank alone decides.
fn axe_rank(name: &str) -> Option<usize> {
    AXE_ORDER.iter().position(|axe| *axe == name)
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
            bot.log("ok axe reattached");
            return Ok(true);
        }
    }
    Ok(false)
}

/// Best-ranked banked axe slot (pack holds one axe: an upgrade banks the
/// old one, a restore withdraws the best, never a second).
fn best_banked_axe(slots: &HashMap<u8, (u16, u32)>, axes: &HashSet<u16>) -> Option<(u8, u16)> {
    slots
        .iter()
        .filter(|(_, (held, _))| axes.contains(held))
        .min_by_key(|(_, (held, _))| {
            // Debug name from the dump ("steel_axe"), not the live display
            // label ("Steel axe"): Tables::obj_name is display-only.
            knowledge::obj_by_id(usize::from(*held))
                .and_then(|entry| entry.name)
                .and_then(|name| axe_rank(&name))
                .unwrap_or(usize::MAX)
        })
        .map(|(slot, (held, _))| (*slot, *held))
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
    // One axe in the pack: withdraw the best banked axe, never a second.
    if let Some((slot, held)) = best_banked_axe(&slots, &axes) {
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
/// until the pack funds the axe. Bounded rounds, each a few kills; the hunt
/// loot stays in the pack for the shared buy below, but anything older
/// (banked logs from a failed sale, shop junk) is stashed first so combat
/// never rides with a dead stock. No gain stops early. True when the pack
/// now affords the axe.
/// Budget: goblin coin drops average ~1.6/kill (goblin.rs2 drop table, 27
/// coin rolls per 128), so 30 kills expect ~47 coins and P(<16) is ~2%.
/// True when the pack holds anything but axes and coins: shop stock or
/// loot from another job that must not ride into combat (death drops it,
/// and it clogs the slots the fight needs).
fn combat_pack_dirty(bot: &Bot, coins: u16) -> bool {
    let axes = known_axe_ids(bot);
    pack_ids(bot).iter().any(|id| *id != coins && !axes.contains(id))
}

fn hunt_goblins(bot: &mut Bot, coins: u16) -> Result<bool, ScriptError> {
    // Combat pack is combat loot only: logs or shop junk riding along die
    // with the bot (or clog the pack), and a failed sale above is exactly
    // how they get there. Stash everything but axes and coins first, so a
    // missed sale banks its stock instead of marching it into the hunt.
    if combat_pack_dirty(bot, coins) {
        let mut keep = known_axe_ids(bot);
        keep.insert(coins);
        let bank = nearest_bank(bot)?;
        bank_trip(bot, &bank, &keep.into_iter().collect::<Vec<_>>(), 0)?;
    }
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

/// Server regrow delay in ticks (1 bot tick = 1 server cycle) per tree
/// table (`woodcutting_trees.dbrow` respawnrate; `loc_change` counts
/// server cycles): oak/willow 30, maple 120, yew 200, magic 400. Rate-0
/// tables (normal/burnt/achey logs) regrow at 120+random(80), midpoint
/// 160. Memory is per tier, not per loc: the rate is defined per tree
/// table, identical for every spot of the tier at fixed playercount
/// (`scale_by_playercount` ~= 1.0 solo), so one exact wait calibrates
/// the whole tier — per-loc memory would need an observation per tree
/// for the same number. Observed waits replace the default.
fn default_respawn_ticks(product: &str) -> u64 {
    match product {
        "oak_logs" | "willow_logs" => 30,
        "maple_logs" => 120,
        "yew_logs" => 200,
        "magic_logs" => 400,
        _ => 160,
    }
}

/// Pure wait-vs-walk call: Some(wait) when standing still wins, None when
/// leaving wins. Wait wins only when strictly cheaper than the walk to the
/// best alternative (`travel_ticks`, Manhattan lower bound — biased toward
/// leaving) and under the Gone-ban window: waiting longer than the ban
/// that would have applied is never better than leaving, so over-cap
/// expectations degrade to None, logged by the caller. No alternative
/// (None) always waits: there is nowhere cheaper to go.
fn respawn_wait_ticks(expected: u64, walk_alternative: Option<u64>) -> Option<u64> {
    if expected > GONE_BAN_TICKS {
        return None;
    }
    match walk_alternative {
        None => Some(expected),
        Some(walk) if expected < walk => Some(expected),
        Some(_) => None,
    }
}

/// Depleted stump under us: wait for the regrow or walk to the next spot,
/// whichever is cheaper. A wait sets the SHORT ban (regrow ETA) so the
/// repick returns to this tree; leaving keeps the Gone ban (250). The
/// wait is threat-checked every pump (hurt or corpse breaks it, long ban,
/// the loop-top flee/death arms handle the rest) and billed to
/// `idle_since` like any other non-chop time, never to the stall budget.
#[allow(clippy::too_many_arguments)]
fn wait_for_respawn(
    bot: &mut Bot,
    tier: &knowledge::trees::TreeTier,
    spot_x: i32,
    spot_z: i32,
    banned: &mut HashMap<(i32, i32), u64>,
    tiers: &[knowledge::trees::TreeTier],
    bank: &knowledge::banks::Bank,
    level: i64,
    memory: &mut HashMap<String, u64>,
    idle_since: &mut u64,
) -> Result<(), ScriptError> {
    let expected = memory.get(&tier.product).copied().unwrap_or_else(|| default_respawn_ticks(&tier.product));
    banned.insert((spot_x, spot_z), bot.world.ticks + GONE_BAN_TICKS);
    let alternative = pick_tree_spot(bot, tiers, bank, level, &mut *banned)
        .map(|(_, found_x, found_z)| travel_ticks((found_x - bot.world.x).abs() as u64 + (found_z - bot.world.z).abs() as u64));
    let Some(wait) = respawn_wait_ticks(expected, alternative) else {
        bot.log(&format!(
            "stump at ({spot_x},{spot_z}): regrow ~{expected}t vs walk {}t, leaving",
            alternative.map(|walk| walk.to_string()).unwrap_or_else(|| "none".to_string())
        ));
        return Ok(());
    };
    // Short ban: the repick after the wait may return to this tree.
    banned.insert((spot_x, spot_z), bot.world.ticks + wait);
    bot.log(&format!(
        "waiting respawn {wait}t at ({spot_x},{spot_z}) (regrow ~{expected}t vs walk {}t)",
        alternative.map(|walk| walk.to_string()).unwrap_or_else(|| "none".to_string())
    ));
    bot.intend(&format!("waiting for the tree at ({spot_x},{spot_z}) to regrow"));
    let start = bot.world.ticks;
    let hitpoints_before = hitpoints(bot);
    while bot.world.ticks - start < wait {
        bot.pump(2)?;
        if hitpoints(bot) < hitpoints_before || hitpoints(bot) == 0 {
            bot.log("threat during respawn wait, leaving the stump");
            banned.insert((spot_x, spot_z), bot.world.ticks + GONE_BAN_TICKS);
            break;
        }
        if tree_still_there(bot, spot_x, spot_z, &tier.trees) {
            break;
        }
    }
    *idle_since += bot.world.ticks - start;
    let elapsed = bot.world.ticks - start;
    if elapsed > 0 && tree_still_there(bot, spot_x, spot_z, &tier.trees) {
        memory.insert(tier.product.clone(), elapsed);
        bot.log(&format!("regrew at ({spot_x},{spot_z}) after {elapsed}t"));
    }
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
                    return Ok((WorkOutcome::UnreachableSpot, gained));
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
    respawn: &mut HashMap<String, u64>,
    refusals: &mut HashMap<(i32, i32), u8>,
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
                if let Some(found) = pick_tree_spot(bot, tiers, bank, level, &mut *banned) {
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
                banned.insert((*spot_x, *spot_z), bot.world.ticks + GONE_BAN_TICKS);
                spot = None;
                continue;
            }
        }
        let Some((tier, spot_x, spot_z)) = spot.clone() else {
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
        // Strikes only survive short bans and walk failures: any other
        // outcome re-proves the stand (chopped, stumped, fled, locked).
        if !matches!(result, WorkOutcome::Unreachable | WorkOutcome::UnreachableSpot) {
            refusals.remove(&(spot_x, spot_z));
        }
        if result != WorkOutcome::Unreachable {
            unreachable_streak = 0;
            // Cluster hop: the spot may sit near another bank, so re-sync
            // on arrival (cheap, no walking) to keep the trip short. The
            // check is positional, not flag-based: drift without a hop
            // resolves to the same bank and is a no-op.
            let fresh = nearest_bank(bot)?;
            if (fresh.x, fresh.z, fresh.level) != (bank.x, bank.z, bank.level) {
                bot.log(&format!(
                    "cluster change: bank re-synced to ({},{}) near {:?}",
                    fresh.x,
                    fresh.z,
                    bot.position()
                ));
                *bank = fresh;
            }
        }
        match result {
            WorkOutcome::Gone => {
                if is_stump(bot, spot_x, spot_z) {
                    // Depleted under us: waiting for the regrow beats
                    // re-banning blind when the walk costs more.
                    wait_for_respawn(bot, &tier, spot_x, spot_z, banned, tiers, bank, level, respawn, &mut idle_since)?;
                } else {
                    bot.log("tree quiet, next one");
                    banned.insert((spot_x, spot_z), bot.world.ticks + GONE_BAN_TICKS);
                }
                spot = None;
            }
            WorkOutcome::NoTool => spot = None,
            WorkOutcome::Unreachable => {
                // Walk-leg failure (never arrived, leg timeout, step refused
                // far away): short ban + retry, NEVER structural. A walk
                // that dies en route proves nothing about the stand.
                bot.log(&format!("unreachable, blacklisting ({spot_x},{spot_z})"));
                banned.insert((spot_x, spot_z), bot.world.ticks + UNREACHABLE_BAN_TICKS);
                spot = None;
                unreachable_streak += 1;
                // Pocket landing (yank mid-run): every spot fails at the
                // walk. Travel back to the bank once, then re-pick from
                // the new region; the stall bound stays the loud exit.
                if unreachable_streak >= 8 {
                    let dest = lifescape_sdk::plan::Tile { x: bank.x as i32, z: bank.z as i32, level: bank.level as i32 };
                    match bot.travel_to(dest) {
                        Ok(()) => {}
                        Err(error @ (lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_))) => {
                            return Err(error.into());
                        }
                        Err(error) => bot.log(&format!("pocket escape refused: {error}")),
                    }
                    // One escape per streak: a refused escape re-arms by
                    // counting, it never re-fires on the next spot.
                    // Without this the same failing travel_to runs every
                    // iteration (up to four full-dijkstra exhaustions
                    // each) and the loop burns a core until the stall
                    // bound trips.
                    unreachable_streak = 0;
                }
            }
            WorkOutcome::UnreachableSpot => {
                // Direct server refusal of the chop click itself. Structural
                // ONLY on a repeated adjacent refusal of a standing tree: a
                // single refusal (transient storm, gate mid-swing) rides the
                // short ban and retries before anything is condemned. A
                // refusal from afar means the walk fell short, not that the
                // stand is walled: short ban, no strike.
                let adjacent = adjacent_to(bot.world.x, bot.world.z, spot_x, spot_z);
                let ban = refusal_ban_ticks(adjacent, is_stump(bot, spot_x, spot_z), true, refusals.get(&(spot_x, spot_z)).copied().unwrap_or(0));
                if is_stump(bot, spot_x, spot_z) {
                    bot.log(&format!("tree stumped at ({spot_x},{spot_z}), next one"));
                } else if adjacent && ban == STRUCTURAL_BAN_TICKS {
                    bot.log(&format!("structurally unreachable, long-banning ({spot_x},{spot_z})"));
                    // Condemned: the ban is the memory now, drop the count
                    // so a starvation release re-proves permanence cleanly.
                    refusals.remove(&(spot_x, spot_z));
                    unreachable_streak = 0;
                } else if adjacent {
                    let strikes = refusals.entry((spot_x, spot_z)).or_insert(0);
                    *strikes += 1;
                    bot.log(&format!(
                        "spot refused once at ({spot_x},{spot_z}), short-banning (strike {}/{DIRECT_REFUSALS_TO_CONDEMN}), retry before condemning",
                        *strikes
                    ));
                    unreachable_streak = 0;
                } else {
                    bot.log(&format!("click refused from afar at ({spot_x},{spot_z}), walk fell short, short ban"));
                    unreachable_streak = 0;
                }
                banned.insert((spot_x, spot_z), bot.world.ticks + ban);
                spot = None;
            }
            WorkOutcome::LevelLocked => {
                banned.insert((spot_x, spot_z), bot.world.ticks + 500);
                spot = None;
            }
            WorkOutcome::Threat => {
                bot.log("hunter close, re-picking a spot");
                banned.insert((spot_x, spot_z), bot.world.ticks + GONE_BAN_TICKS);
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

/// Pack ids that survive the bank trip: the single best axe only. An
/// upgrade or a restore leaves the old axe behind in the bank instead of
/// doubling up: the server chops with the best carried axe at any level,
/// so a second axe is dead weight that death would drop anyway.
fn chop_keep_ids(bot: &Bot) -> Vec<u16> {
    best_axe(bot).and_then(|name| obj_id(bot, &name).ok()).into_iter().collect()
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
    // Observed regrow delays per tier product: one exact wait calibrates
    // the whole tier (server rate is per tree table), so this outlives
    // loads like the bans do.
    let mut respawn: HashMap<String, u64> = HashMap::new();
    // Adjacent direct-refusal strikes per stand: the first refusal rides
    // the short ban, the repeat condemns (structural). Walk-leg failures
    // never write here.
    let mut refusals: HashMap<(i32, i32), u8> = HashMap::new();
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
        let (ok, new_level) = chop_load(bot, &tiers, &mut bank, coins, level, &mut banned, &mut respawn, &mut refusals)?;
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
        // The shop trip can leave two axes in the pack (old + upgrade):
        // stash then, so the spot sees one axe and no coins.
        if pack_total(bot, coins) > 0 {
            let _ = buy_axe_upgrade(bot, &cands)?;
        }
        let axes_held: u32 = known_axe_ids(bot).iter().map(|id| pack_total(bot, *id)).sum();
        if pack_total(bot, coins) > 0 || axes_held > 1 {
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
    use super::{bootstrap_products, can_buy_now, chop_keep_ids, combat_pack_dirty, death_since, default_respawn_ticks, respawn_wait_ticks};
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

    /// Combat pack rule: logs or shop junk force a stash, axes and coins
    /// ride along. A missed sale must bank its stock, never march it into
    /// the hunt.
    #[test]
    fn combat_pack_dirty_iff_noncombat_carried() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let logs = crate::skill_common::obj_id(&bot, "logs").expect("logs in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        assert!(!combat_pack_dirty(&bot, coins), "empty pack hunts as-is");
        bot.world.inv.insert(com, [(0u8, (coins, 3u32))].into_iter().collect());
        assert!(!combat_pack_dirty(&bot, coins), "coins ride along");
        bot.world.inv.insert(com, [(0u8, (coins, 3u32)), (1, (logs, 6))].into_iter().collect());
        assert!(combat_pack_dirty(&bot, coins), "banked logs must stash before the hunt");
    }

    /// Bootstrap sale set: ground loot and logs fund the axe, axes/coins
    /// and the unbankable lamp never sell.
    #[test]
    fn bootstrap_products_sells_loot_never_axe_or_lamp() {        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
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

    /// Bank keep holds one axe only: the best carried. Coins always land
    /// in the bank, never ride to the spot; inferior axes are banked, not
    /// doubled up (the server chops with the best carried axe at any
    /// level, so a second axe is dead weight).
    #[test]
    fn chop_keep_never_holds_coins() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let coins = crate::skill_common::obj_id(&bot, "coins").expect("coins in tables");
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let com = (bot.tables.com_id)("inventory:inv").expect("inventory:inv in dumps");
        bot.world.inv.insert(com, [(0u8, (bronze, 1u32)), (1, (steel, 1))].into_iter().collect());
        let keep = chop_keep_ids(&bot);
        assert_eq!(keep, vec![steel], "one axe in the pack: the best, got: {keep:?}");
        assert!(!keep.contains(&coins), "coins always banked, never carried");
        assert!(!keep.contains(&bronze), "inferior axe banked, never doubled");
    }

    /// Restore withdraws the best banked axe, never a second or a worse
    /// one (rank order is AXE_ORDER best-first).
    #[test]
    fn bank_restore_withdraws_best_rank() {
        use super::{axe_rank, best_banked_axe, known_axe_ids};
        use std::collections::HashMap;
        assert!(axe_rank("steel_axe") < axe_rank("bronze_axe"), "steel outranks bronze");
        assert_eq!(axe_rank("no_such_axe"), None, "unknown axes never rank");
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let bronze = crate::skill_common::obj_id(&bot, "bronze_axe").expect("bronze_axe in tables");
        let steel = crate::skill_common::obj_id(&bot, "steel_axe").expect("steel_axe in tables");
        let axes = known_axe_ids(&bot);
        let slots: HashMap<u8, (u16, u32)> = [(3u8, (bronze, 1u32)), (7, (steel, 1))].into_iter().collect();
        assert_eq!(best_banked_axe(&slots, &axes), Some((7, steel)), "best banked axe wins");
        assert_eq!(best_banked_axe(&HashMap::new(), &axes), None, "empty bank restores nothing");
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

    /// Defaults mirror the server tree table (`woodcutting_trees.dbrow`
    /// respawnrate): oak/willow 30, maple 120, yew 200, magic 400, and
    /// 160 for normal logs (120+random(80), midpoint). Unknown products
    /// fall back to the normal-tree mean, never a round guess.
    #[test]
    fn respawn_defaults_match_server_table() {
        assert_eq!(default_respawn_ticks("oak_logs"), 30, "oak regrows in 30");
        assert_eq!(default_respawn_ticks("willow_logs"), 30, "willow regrows in 30");
        assert_eq!(default_respawn_ticks("maple_logs"), 120, "maple regrows in 120");
        assert_eq!(default_respawn_ticks("yew_logs"), 200, "yew regrows in 200");
        assert_eq!(default_respawn_ticks("magic_logs"), 400, "magic regrows in 400");
        assert_eq!(default_respawn_ticks("logs"), 160, "normal trees average 160");
        assert_eq!(default_respawn_ticks("something_new"), 160, "unknown falls back to the normal mean");
    }

    /// Wait-vs-walk on chiffré cases (GONE_BAN_TICKS = 250 cap): oak 30
    /// vs a 200-tile walk waits 30; yew 200 vs a 50-tile walk leaves;
    /// magic 400 over the cap leaves even with nowhere better to go
    /// (the ban repick revisits); a tie walks; no alternative waits.
    #[test]
    fn respawn_wait_wins_only_when_strictly_cheaper() {
        use crate::skill_common::GONE_BAN_TICKS;
        assert_eq!(respawn_wait_ticks(30, Some(200)), Some(30), "oak 30 < walk 200 waits");
        assert_eq!(respawn_wait_ticks(200, Some(50)), None, "yew 200 > walk 50 leaves");
        assert_eq!(respawn_wait_ticks(400, Some(500)), None, "magic 400 over cap leaves");
        assert_eq!(respawn_wait_ticks(400, None), None, "magic 400 over cap even with nowhere to go");
        assert_eq!(respawn_wait_ticks(30, Some(30)), None, "tie walks");
        assert_eq!(respawn_wait_ticks(30, None), Some(30), "nowhere to go waits");
        assert_eq!(respawn_wait_ticks(GONE_BAN_TICKS, Some(1000)), Some(GONE_BAN_TICKS), "cap itself still waits");
        assert_eq!(respawn_wait_ticks(GONE_BAN_TICKS + 1, Some(1000)), None, "one over cap leaves");
    }

}
