//! `market cleanup`: thin bank janitor (L8). The brain passes `--keep`
//! (graph obtains, quest checks) and `--iron` (D2 direct, no carnet); coins,
//! food to 100 units and the best banked tool per kit spec resolve here.
//! One pass: market sells for tradeable surplus (skipped when `--iron`),
//! general-store sale, drop the rest (D10); a bank still past the limit
//! drops whole listed stacks cheapest-total first with orders cancelled
//! (D17). Sells list at the reserve floor at once: step-down across runs
//! is deferred (no order age in `my_orders` to pace it).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_sdk::Bot;

use crate::kit::{bank::nearest_bank, pack::{free_slots, obj_id, pack_total}, tools::cleanup_keep_names};
use crate::market_client::{cancel_for, open_sell_items, place_sell};
use crate::{ScriptError, ScriptResult};

struct CleanupArgs {
    keep: Vec<String>,
    iron: bool,
    budget_max: u64,
}

fn parse_args(args: &[String]) -> Result<CleanupArgs, ScriptError> {
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
    for (id, qty) in bot.bank_items().iter() {
        if *qty == 0 {
            continue;
        }
        if let Some(name) = lifescape_gamedata::obj_by_id(usize::from(*id)).and_then(|entry| entry.name) {
            rows.push((*id, name.to_string()));
        }
    }
    rows
}

fn spent(start_tick: u64, start_pause: u64, bot: &Bot) -> u64 {
    bot.world.ticks.saturating_sub(start_tick).saturating_sub(bot.pause_ticks.saturating_sub(start_pause))
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
        let qty = bot.bank_items().get(id).copied().unwrap_or(0);
        if qty > 0 {
            let _ = bot.bank_withdraw(*id, qty, 300);
        }
    }
    let _ = bot.close_modal();
    bot.pump(3)?;
    Ok(())
}

fn packed(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    ids.iter().filter(|id| bot.inventory().get(id).copied().unwrap_or(0) > 0).copied().collect()
}

// Pack-sized withdrawal slice of banked ids: the pack holds 28, so every
// pass lists, sells and drops in chunks instead of stalling full.
fn next_chunk(bot: &Bot, ids: &[u16]) -> Vec<u16> {
    let mut chunk = Vec::new();
    let mut free = free_slots(bot);
    for id in ids {
        if bot.bank_items().get(id).copied().unwrap_or(0) == 0 || free == 0 {
            continue;
        }
        chunk.push(*id);
        free -= 1;
    }
    chunk
}

fn drop_all(bot: &mut Bot, id: u16, start_tick: u64, start_pause: u64, budget_max: u64) -> Result<bool, ScriptError> {
    while bot.inventory().get(&id).copied().unwrap_or(0) > 0 {
        if spent(start_tick, start_pause, bot) >= budget_max {
            return Ok(false);
        }
        // Server truth beats a stale pack cache: already gone counts
        // as dropped, never as a failed run.
        match bot.drop(id, 750) {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) if error.to_string().contains("no such item") => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

/// One janitor pass: exit 0 when the bank is back under the limit,
/// budget exit when more passes could still help.
pub fn run_cleanup(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let parsed = parse_args(args)?;
    let (start_tick, start_pause) = (bot.world.ticks, bot.pause_ticks);
    // Triage reads the live bank dialog: open first, since a closed
    // dialog reads stale (or empty) and would mis-sort the whole pass.
    bank_open(bot)?;
    let coins = obj_id(bot, "coins")?;
    let keep = [parsed.keep.clone(), vec!["coins".to_string()], cleanup_keep_names(bot, &bot.bank_items().keys().copied().collect())].concat();
    // Unknown keeps fail loud: silence would sell what the brain forbade.
    for name in &keep {
        let _ = obj_id(bot, name)?;
    }
    let named = bank_names(bot);
    let stacks: Vec<(String, i64)> = named.iter().map(|(held, name)| (name.clone(), i64::from(bot.bank_items().get(held).copied().unwrap_or(0)))).collect();
    let (protected, surplus) = lifescape_planner::space::triage_bank(&stacks, &keep);
    bot.log(&format!("cleanup: {} protected, {} surplus", protected.len(), surplus.len()));
    if surplus.is_empty() {
        return Ok(0);
    }
    let by_name: HashMap<&str, u16> = named.iter().map(|(id, name)| (name.as_str(), *id)).collect();
    let surplus_ids: Vec<u16> = surplus.iter().filter_map(|(name, _)| by_name.get(name.as_str()).copied()).collect();
    // Carnet stock leaves the later legs alone: the shop must never sell
    // what the market reserved. Chunked: the pack holds 28, so list the
    // carried stock, then withdraw and list the banked remainder. Already
    // listed stock is skipped, never stacked: a relist would double-spend
    // the ledger reservation, and the D17 gate below reads "nothing new".
    let (mut market_ids, mut listed_now): (Vec<u16>, u32) = (Vec::new(), 0);
    let listed_open = if parsed.iron { HashSet::new() } else { open_sell_items(bot) };
    if !parsed.iron {
        for id in &surplus_ids {
            let listed = lifescape_gamedata::obj_by_id(usize::from(*id))
                .is_some_and(|entry| entry.tradeable && entry.members != Some(true));
            if listed {
                market_ids.push(*id);
            }
        }
        let mut todo: Vec<u16> = market_ids.iter().filter(|id| !listed_open.contains(id)).copied().collect();
        loop {
            for id in packed(bot, &todo) {
                if spent(start_tick, start_pause, bot) >= parsed.budget_max {
                    return Ok(crate::kit::cycle::BUDGET_EXIT);
                }
                let held = i64::from(bot.inventory().get(&id).copied().unwrap_or(0));
                let price = lifescape_gamedata::obj_by_id(usize::from(id)).map(|entry| entry.price).unwrap_or(1);
                // Reserve floor at once (general-store buyback estimate); the
                // passthrough alchemy win stays planner-side until a magic
                // script can cast it.
                if place_sell(bot, id, held, lifescape_planner::space::reserve_floor(price * 400 / 1000, None))? {
                    listed_now += 1;
                }
                todo.retain(|keep| keep != &id);
            }
            let chunk = next_chunk(bot, &todo);
            if chunk.is_empty() { break; }
            withdraw_all(bot, &chunk)?;
        }
    }
    // Shop pass on the non-carnet rest, chunked like the market pass: the
    // probe inside keeps worthless stock, dropped at once per chunk (D10).
    let mut rest: Vec<u16> = surplus_ids.iter().filter(|id| !market_ids.contains(id)).copied().collect();
    loop {
        let carried = packed(bot, &rest);
        if !carried.is_empty() {
            match crate::kit::shop::sell_for_target(bot, &carried, coins, u32::MAX - pack_total(bot, coins)) {
                Ok(gained) => bot.log(&format!("cleanup shop sold for {gained}")),
                Err(error) => bot.log(&format!("cleanup shop pass skipped ({error}), dropping leftovers")),
            }
            for id in packed(bot, &rest) {
                if !drop_all(bot, id, start_tick, start_pause, parsed.budget_max)? {
                    return Ok(crate::kit::cycle::BUDGET_EXIT);
                }
            }
            if !packed(bot, &rest).is_empty() {
                break;
            }
            rest.retain(|id| bot.bank_items().get(id).copied().unwrap_or(0) > 0);
        }
        let chunk = next_chunk(bot, &rest);
        if chunk.is_empty() { break; }
        withdraw_all(bot, &chunk)?;
    }
    // D17: a still-full bank with nothing newly listed drops whole listed
    // stacks cheapest-total first, after cancelling their orders. Fresh
    // listings get one full pass to fill (the next pass lists nothing new
    // and escalates); unlisted surplus re-lists instead of bypassing the
    // carnet (D12). Counts read with the bank open: closed reads lie.
    bank_open(bot)?;
    if bot.bank_items().values().filter(|qty| **qty > 0).count() >= lifescape_planner::space::BANK_TYPE_LIMIT && listed_now == 0 {
        let sheltered: HashSet<u16> = protected.iter().filter_map(|name| by_name.get(name.as_str()).copied()).collect();
        let mut stuck = Vec::new();
        for (id, name) in &named {
            let qty = i64::from(bot.bank_items().get(id).copied().unwrap_or(0));
            if !sheltered.contains(id) && listed_open.contains(id) && qty > 0 {
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
            // Pack first (a clogged pack blocks the withdraw), then the
            // banked stack, then the pack again: whole stacks go. The
            // stop check reads warm (the withdraw just opened the bank).
            drop_all(bot, id, start_tick, start_pause, parsed.budget_max)?;
            if bot.bank_items().get(&id).copied().unwrap_or(0) == 0 {
                continue;
            }
            withdraw_all(bot, &[id])?;
            let done = bot.bank_items().values().filter(|qty| **qty > 0).count() < lifescape_planner::space::BANK_TYPE_LIMIT;
            drop_all(bot, id, start_tick, start_pause, parsed.budget_max)?;
            bot.log(&format!("cleanup D17 dropped {name}"));
            if done {
                break;
            }
        }
    }
    bank_open(bot)?;
    let types = bot.bank_items().values().filter(|qty| **qty > 0).count();
    bot.log(&format!("cleanup done: {types} bank types"));
    Ok(if types < lifescape_planner::space::BANK_TYPE_LIMIT { 0 } else { crate::kit::cycle::BUDGET_EXIT })
}

#[cfg(test)]
mod tests {
    use super::parse_args;

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
