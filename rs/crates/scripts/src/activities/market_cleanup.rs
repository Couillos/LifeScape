//! `market cleanup`: thin bank janitor (L8). The brain passes `--keep`
//! (graph obtains, quest checks) and `--iron` (D2 direct, no carnet); coins,
//! food to 100 units and the best banked tool per kit spec resolve here.
//! One pass: market sells for tradeable surplus (skipped when `--iron`),
//! drop the rest (D10; PRD3: never an NPC shop sale); a bank still past
//! the limit drops whole stacks cheapest-total first with orders
//! cancelled (D17) until it is back under: listed stock, and the surplus
//! the carnet refuses (no own cost known), which never sells, so only its
//! drop frees the slot. Sells open at the market and walk down to the
//! reserve floor daemon-side (`market_reprice`).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_sdk::Bot;

use crate::kit::janitor::{drop_all, known_bank, known_bank_types, next_chunk, packed, spent};
use crate::kit::{bank::nearest_bank, pack::obj_id, tools::cleanup_keep_names};
use crate::market_client::{cancel_for, open_sell_items, place_sell};
use crate::{ScriptError, ScriptResult};

pub(crate) struct CleanupArgs {
    keep: Vec<String>,
    iron: bool,
    budget_max: u64,
}

pub(crate) fn parse_args(args: &[String]) -> Result<CleanupArgs, ScriptError> {
    let usage = || ScriptError::Usage("market cleanup [--keep NAME]... [--iron] [--node ID] [--budget-max N]".to_string());
    let mut keep = Vec::new();
    let mut iron = false;
    let mut budget_max = 6000u64;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--keep" => {
                keep.push(args.get(index + 1).filter(|name| !name.is_empty()).ok_or_else(usage)?.clone()); index += 2;
            }
            "--iron" => { iron = true; index += 1; }
            "--node" | "--budget-min" => index += 2,
            "--budget-max" => {
                budget_max = args.get(index + 1).and_then(|raw| raw.parse().ok()).ok_or_else(usage)?; index += 2;
            }
            _ => return Err(usage()),
        }
    }
    Ok(CleanupArgs { keep, iron, budget_max })
}

// Debug names in the bank; unknown ids never list, so never sell.
fn bank_names(bot: &Bot) -> Vec<(u16, String)> {
    let mut rows = Vec::new();
    for (id, qty) in known_bank(bot).iter() {
        if *qty == 0 {
            continue;
        }
        if let Some(name) = lifescape_gamedata::obj_by_id(usize::from(*id)).and_then(|entry| entry.name) {
            rows.push((*id, name.to_string()));
        }
    }
    rows
}

fn bank_open(bot: &mut Bot) -> Result<bool, ScriptError> {
    let home = nearest_bank(bot).map_err(|_| ScriptError::Blocked { missing: vec!["bank".to_string()] })?;
    if !bot.open_bank(home.x as i32, home.z as i32, home.loc_id as u16, 300)? {
        return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
    }
    Ok(true)
}

fn withdraw_all(bot: &mut Bot, ids: &[u16]) -> Result<(), ScriptError> {
    bank_open(bot)?;
    for id in ids {
        let qty = known_bank(bot).get(id).copied().unwrap_or(0);
        if qty > 0 {
            let _ = bot.bank_withdraw(*id, qty, 300);
        }
    }
    let _ = bot.close_modal();
    bot.pump(3)?;
    Ok(())
}

/// Drops the cheapest carried stack outside `sheltered` (and not
/// `target`, the stack about to be withdrawn) when the pack is full.
fn free_one_slot(bot: &mut Bot, sheltered: &HashSet<u16>, target: u16) -> Result<(), ScriptError> {
    if crate::kit::pack::free_slots(bot) > 0 {
        return Ok(());
    }
    let coins = obj_id(bot, "coins").ok();
    let victim = crate::kit::pack::pack_slots(bot)
        .iter()
        .map(|(_, (held, count))| (*held, *count))
        .filter(|(held, _)| !sheltered.contains(held) && *held != target && Some(*held) != coins && !crate::kit::pack::is_worn(bot, *held))
        .min_by_key(|(held, count)| lifescape_gamedata::obj_by_id(usize::from(*held)).map(|entry| entry.price.max(1)).unwrap_or(1) * i64::from(*count));
    let Some((held, _)) = victim else {
        bot.log("cleanup D17: the pack is full of protected stacks, nothing to drop");
        return Ok(());
    };
    bot.log(&format!("cleanup D17: dropping {} to free a pack slot", crate::kit::pack::obj_label(bot, held)));
    bot.drop(held, 750)?;
    Ok(())
}

/// D17 candidate (pure): surplus the carnet refused this pass (it can
/// never sell, only its drop frees the slot) and stock resting listed
/// since an earlier pass. Listings placed this pass are not in
/// `listed_open`: they get this pass to fill. The old rule also spared
/// the resting listings whenever anything new listed (live ryan_park
/// 2026-11-15: each pass listed 3 fresh stacks out of 46 surplus, D17
/// never ran, the bank sat at 60 types and every load failed "bank full"
/// 151 times in two hours).
fn d17_eligible(id: u16, unsellable: &[u16], listed_open: &HashSet<u16>) -> bool {
    unsellable.contains(&id) || listed_open.contains(&id)
}

/// One janitor pass: exit 0 when the bank is back under the limit,
/// budget exit when more passes could still help.
pub fn run_cleanup(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let parsed = parse_args(args)?;
    let (start_tick, start_pause) = (bot.world.ticks, bot.pause_ticks);
    // Triage reads the live bank dialog: open first, since a closed
    // dialog reads stale (or empty) and would mis-sort the whole pass.
    bank_open(bot)?;
    obj_id(bot, "coins")?;
    let keep = [parsed.keep.clone(), vec!["coins".to_string()], cleanup_keep_names(bot, &bot.bank_items().keys().copied().collect())].concat();
    // Unknown keeps fail loud: silence would sell what the brain forbade.
    for name in &keep {
        let _ = obj_id(bot, name)?;
    }
    let named = bank_names(bot);
    let bank = known_bank(bot);
    let stacks: Vec<(String, i64)> = named.iter().map(|(held, name)| (name.clone(), i64::from(bank.get(held).copied().unwrap_or(0)))).collect();
    let (protected, surplus) = lifescape_planner::space::triage_bank(&stacks, &keep);
    bot.log(&format!("cleanup: {} protected, {} surplus", protected.len(), surplus.len()));
    if surplus.is_empty() {
        return Ok(0);
    }
    let by_name: HashMap<&str, u16> = named.iter().map(|(id, name)| (name.as_str(), *id)).collect();
    let surplus_ids: Vec<u16> = surplus.iter().filter_map(|(name, _)| by_name.get(name.as_str()).copied()).collect();
    // Carnet stock leaves the later legs alone: the shop must never sell
    // what the market reserved. Already listed stock is skipped, never stacked: a relist would double-spend
    // the ledger reservation, and the D17 gate below reads "nothing new".
    let mut market_ids: Vec<u16> = Vec::new();
    // Tradeable surplus the carnet refused this pass: it never lists, so
    // only D17 below can free its slot.
    let mut unsellable: Vec<u16> = Vec::new();
    let listed_open = if parsed.iron { HashSet::new() } else { open_sell_items(bot) };
    if !parsed.iron {
        for id in &surplus_ids {
            let listed = lifescape_gamedata::obj_by_id(usize::from(*id))
                .is_some_and(|entry| entry.tradeable && entry.members != Some(true));
            if listed {
                market_ids.push(*id);
            }
        }
        let todo: Vec<u16> = market_ids.iter().filter(|id| !listed_open.contains(id)).copied().collect();
        // Listed where it lies (pack plus bank: the carnet reserves banked
        // stock, the deal withdraws it), never withdrawn for nothing. A
        // stack the seller holds (no own cost known, over the band) or the
        // carnet refuses never lists this pass: D17 below frees its slot
        // (live bst29bh9 2026-11-14: twelve holds read as "carried" through
        // `inventory()`, never refused, the bank stayed at 60 types).
        for id in todo {
            if spent(start_tick, start_pause, bot) >= parsed.budget_max {
                return Ok(crate::kit::cycle::BUDGET_EXIT);
            }
            let held = i64::from(bot.pack_count(id)) + i64::from(known_bank(bot).get(&id).copied().unwrap_or(0));
            if held <= 0 {
                continue;
            }
            // Reserve floor: 1 coin (no NPC buyback in the economy);
            // the alchemy win stays planner-side (D2).
            if !place_sell(bot, id, held, lifescape_planner::space::reserve_floor(None))? {
                unsellable.push(id);
            }
        }
    }
    // Non-carnet rest (untradeable surplus, or Iron stock), chunked like
    // the market pass: dropped (D10). PRD3: never sold to an NPC shop.
    let mut rest: Vec<u16> = surplus_ids.iter().filter(|id| !market_ids.contains(id)).copied().collect();
    loop {
        let carried = packed(bot, &rest);
        if !carried.is_empty() {
            for id in packed(bot, &rest) {
                if !drop_all(bot, id, start_tick, start_pause, parsed.budget_max)? {
                    return Ok(crate::kit::cycle::BUDGET_EXIT);
                }
            }
            if !packed(bot, &rest).is_empty() {
                break;
            }
            rest.retain(|id| known_bank(bot).get(id).copied().unwrap_or(0) > 0);
        }
        let chunk = next_chunk(bot, &rest);
        if chunk.is_empty() { break; }
        withdraw_all(bot, &chunk)?;
        // A withdrawal that freed nothing (stack locked) ends the leg
        // instead of looping on it.
        if packed(bot, &chunk).is_empty() { break; }
    }
    // D17: a still-full bank drops whole stacks cheapest-total first,
    // after cancelling their orders, until it is back under the limit:
    // the surplus the carnet refused this pass (it never lists) and the
    // stock resting listed since an earlier pass (`d17_eligible`): this
    // pass's fresh listings get the pass to fill. Counts read the last
    // known bank (`known_bank`).
    bank_open(bot)?;
    if known_bank_types(bot) >= lifescape_planner::space::BANK_TYPE_LIMIT {
        let sheltered: HashSet<u16> = protected.iter().filter_map(|name| by_name.get(name.as_str()).copied()).collect();
        let mut stuck = Vec::new();
        for (id, name) in &named {
            let qty = i64::from(known_bank(bot).get(id).copied().unwrap_or(0));
            if !sheltered.contains(id) && qty > 0 && d17_eligible(*id, &unsellable, &listed_open) {
                let price = lifescape_gamedata::obj_by_id(usize::from(*id)).map(|entry| entry.price.max(1)).unwrap_or(1);
                stuck.push((name.clone(), qty, price));
            }
        }
        for name in lifescape_planner::space::d17_order(&stuck) {
            if spent(start_tick, start_pause, bot) >= parsed.budget_max {
                return Ok(crate::kit::cycle::BUDGET_EXIT);
            }
            let Some(id) = by_name.get(name.as_str()).copied() else { continue };
            cancel_for(bot, id)?;
            // A full pack takes no withdrawal (live sean_lane 2026-11-15:
            // 28 slots the full bank refused, every D17 stack "still
            // locked" after its order was cancelled): the cheapest
            // unprotected carried stack goes first, by the same rule.
            free_one_slot(bot, &sheltered, id)?;
            // Pack first (a clogged pack blocks the withdraw), then the
            // banked stack pack by pack (an unnoted stack outgrows one
            // pack), each load dropped: whole stacks go. A withdraw that
            // frees nothing (still locked by an order) ends the stack.
            // Counts read the last known bank: the withdraw closes the
            // dialog, a live read would say "cleared" every time.
            if !drop_all(bot, id, start_tick, start_pause, parsed.budget_max)? {
                return Ok(crate::kit::cycle::BUDGET_EXIT);
            }
            loop {
                let banked = known_bank(bot).get(&id).copied().unwrap_or(0);
                if banked == 0 {
                    break;
                }
                withdraw_all(bot, &[id])?;
                let left = known_bank(bot).get(&id).copied().unwrap_or(0);
                if !drop_all(bot, id, start_tick, start_pause, parsed.budget_max)? {
                    return Ok(crate::kit::cycle::BUDGET_EXIT);
                }
                if left >= banked {
                    break;
                }
            }
            if known_bank(bot).get(&id).copied().unwrap_or(0) > 0 {
                bot.log(&format!("cleanup D17 could not clear {name} (still banked: locked, or no pack room)"));
                continue;
            }
            bot.log(&format!("cleanup D17 dropped {name}"));
            if known_bank_types(bot) < lifescape_planner::space::BANK_TYPE_LIMIT {
                break;
            }
        }
    }
    bank_open(bot)?;
    let types = known_bank_types(bot);
    bot.log(&format!("cleanup done: {types} bank types"));
    Ok(if types < lifescape_planner::space::BANK_TYPE_LIMIT { 0 } else { crate::kit::cycle::BUDGET_EXIT })
}

#[cfg(test)]
mod tests {
    use super::{d17_eligible, parse_args};

    /// Live bdr53jee 2026-11-14: the seller held 13 stacks (no own cost
    /// known), D17 only looked at listed stock, the bank stayed at 56
    /// types every pass. Refused surplus is always a D17 candidate; a
    /// resting listing too, fresh listings beside it or not (ryan_park
    /// 2026-11-15).
    #[test]
    fn refused_surplus_always_escalates_to_d17() {
        let listed: std::collections::HashSet<u16> = [526].into_iter().collect();
        assert!(d17_eligible(561, &[561], &listed), "refused: drop candidate");
        assert!(d17_eligible(526, &[], &listed), "resting listing from an earlier pass");
        assert!(!d17_eligible(1205, &[], &listed), "neither refused nor resting: untouched");
    }

    #[test]
    fn cleanup_args_parse() {
        let args = ["cleanup".to_string(), "--keep".to_string(), "feather".to_string(), "--iron".to_string(), "--node".to_string(), "n9".to_string(), "--budget-max".to_string(), "6000".to_string()];
        let parsed = parse_args(&args).expect("cleanup args parse");
        assert_eq!(parsed.keep, vec!["feather".to_string()]);
        assert!(parsed.iron, "iron flag kept");
        assert_eq!(parsed.budget_max, 6000);
        let parsed = parse_args(&["cleanup".to_string()]).expect("bare cleanup parses");
        assert!(parsed.keep.is_empty() && !parsed.iron, "defaults: no keep, market allowed");
        // Exact brain order shape: node + engagement window ride along.
        let brain = ["cleanup".to_string(), "--node".to_string(), "n2".to_string(), "--budget-min".to_string(), "0".to_string(), "--budget-max".to_string(), "6000".to_string()];
        let parsed = parse_args(&brain).expect("brain order parses");
        assert_eq!(parsed.budget_max, 6000, "window kept");
        assert!(parse_args(&["cleanup".to_string(), "--frobnicate".to_string()]).is_err(), "junk flags fail loud");
    }
}
