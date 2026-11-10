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

use crate::kit::tools::{AXE_SPEC, FISH_SPEC, PICK_SPEC};
use crate::market_client::{cancel_for, open_sell_items, place_sell};
use crate::skill_common::{nearest_bank, obj_id, pack_total};
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
                keep.push(args.get(index + 1).filter(|name| !name.is_empty()).ok_or_else(usage)?.clone());
                index += 2;
            }
            "--iron" => {
                iron = true;
                index += 1;
            }
            "--node" => index += 2,
            "--budget-max" => {
                budget_max = args.get(index + 1).and_then(|raw| raw.parse().ok()).ok_or_else(usage)?;
                index += 2;
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

// Best banked tool per kit spec, plus tinderbox.
fn tool_keep(bot: &Bot, banked: &HashSet<u16>) -> Vec<String> {
    let mut keep = Vec::new();
    for spec in [&AXE_SPEC, &PICK_SPEC, &FISH_SPEC] {
        let mut best: Option<(usize, &str)> = None;
        for name in spec.order {
            let rank = spec.rank(name).unwrap_or(usize::MAX);
            let held = obj_id(bot, name).is_ok_and(|id| banked.contains(&id));
            if held && best.is_none_or(|(top, _)| rank < top) {
                best = Some((rank, name));
            }
        }
        if let Some((_, name)) = best {
            keep.push(name.to_string());
        }
    }
    if obj_id(bot, "tinderbox").is_ok_and(|id| banked.contains(&id)) {
        keep.push("tinderbox".to_string());
    }
    keep
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

fn drop_all(bot: &mut Bot, id: u16, start_tick: u64, start_pause: u64, budget_max: u64) -> Result<bool, ScriptError> {
    while bot.inventory().get(&id).copied().unwrap_or(0) > 0 {
        if spent(start_tick, start_pause, bot) >= budget_max {
            return Ok(false);
        }
        if !bot.drop(id, 750)? {
            break;
        }
    }
    Ok(true)
}

fn bank_types(bot: &Bot) -> usize {
    bot.bank_items().values().filter(|qty| **qty > 0).count()
}

/// One janitor pass: exit 0 when the bank is back under the limit,
/// budget exit when more passes could still help.
pub fn run_cleanup(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let parsed = parse_args(args)?;
    let (start_tick, start_pause) = (bot.world.ticks, bot.pause_ticks);
    let coins = obj_id(bot, "coins")?;
    let mut keep = [parsed.keep.clone(), vec!["coins".to_string()]].concat();
    keep.extend(tool_keep(bot, &bot.bank_items().keys().copied().collect()));
    // Unknown keeps fail loud: silence would sell what the brain forbade.
    for name in &keep {
        let _ = obj_id(bot, name)?;
    }
    let named = bank_names(bot);
    let qtys: HashMap<u16, u32> = bot.bank_items().clone();
    let stacks: Vec<(String, i64)> =
        named.iter().map(|(id, name)| (name.clone(), i64::from(*qtys.get(id).unwrap_or(&0)))).collect();
    let (protected, surplus) = lifescape_planner::space::triage_bank(&stacks, &keep);
    bot.log(&format!("cleanup: {} protected, {} surplus", protected.len(), surplus.len()));
    if surplus.is_empty() {
        return Ok(0);
    }
    let by_name: HashMap<&str, u16> = named.iter().map(|(id, name)| (name.as_str(), *id)).collect();
    let surplus_ids: Vec<u16> = surplus.iter().filter_map(|(name, _)| by_name.get(name.as_str()).copied()).collect();
    // Carnet stock leaves the later legs alone: the shop must never sell
    // what the market reserved.
    let mut market_ids = Vec::new();
    if !parsed.iron {
        for id in &surplus_ids {
            let listed = lifescape_gamedata::obj_by_id(usize::from(*id))
                .is_some_and(|entry| entry.tradeable && entry.members != Some(true));
            if listed {
                market_ids.push(*id);
            }
        }
        if !market_ids.is_empty() {
            withdraw_all(bot, &market_ids)?;
            for id in packed(bot, &market_ids) {
                if spent(start_tick, start_pause, bot) >= parsed.budget_max {
                    return Ok(crate::kit::cycle::BUDGET_EXIT);
                }
                let held = i64::from(bot.inventory().get(&id).copied().unwrap_or(0));
                let price = lifescape_gamedata::obj_by_id(usize::from(id)).map(|entry| entry.price).unwrap_or(1);
                // Reserve floor at once (general-store buyback estimate); the
                // passthrough alchemy win stays planner-side until a magic
                // script can cast it.
                place_sell(bot, id, held, lifescape_planner::space::reserve_floor(price * 400 / 1000, None))?;
            }
        }
    }
    // Shop pass on the non-carnet rest; the probe inside keeps worthless
    // stock for the drop leg.
    let rest_ids: Vec<u16> = surplus_ids.iter().filter(|id| !market_ids.contains(id)).copied().collect();
    withdraw_all(bot, &rest_ids)?;
    let carried = packed(bot, &rest_ids);
    if !carried.is_empty() {
        let before = pack_total(bot, coins);
        match crate::skill_trade::sell_for_target(bot, &carried, coins, u32::MAX - before) {
            Ok(gained) => bot.log(&format!("cleanup shop sold for {gained}")),
            Err(error) => bot.log(&format!("cleanup shop pass skipped ({error}), dropping leftovers")),
        }
    }
    // Drop leg (D10): what no shop buys and no carnet takes.
    for id in packed(bot, &rest_ids) {
        if !drop_all(bot, id, start_tick, start_pause, parsed.budget_max)? {
            return Ok(crate::kit::cycle::BUDGET_EXIT);
        }
    }
    // D17: a still-full bank drops whole listed stacks cheapest-total
    // first, after cancelling their orders. Unlisted surplus re-lists on
    // the next pass instead of bypassing the carnet (D12).
    let _ = bot.pump(3)?;
    if bank_types(bot) >= lifescape_planner::space::BANK_TYPE_LIMIT {
        let listed = open_sell_items(bot);
        let sheltered: HashSet<u16> = protected.iter().filter_map(|name| by_name.get(name.as_str()).copied()).collect();
        let mut stuck = Vec::new();
        for (id, name) in &named {
            if sheltered.contains(id) || !listed.contains(id) {
                continue;
            }
            let qty = i64::from(bot.bank_items().get(id).copied().unwrap_or(0));
            let price = lifescape_gamedata::obj_by_id(usize::from(*id)).map(|entry| entry.price.max(1)).unwrap_or(1);
            if qty > 0 {
                stuck.push((name.clone(), qty, price));
            }
        }
        for name in lifescape_planner::space::d17_order(&stuck) {
            if spent(start_tick, start_pause, bot) >= parsed.budget_max {
                return Ok(crate::kit::cycle::BUDGET_EXIT);
            }
            let Some(id) = by_name.get(name.as_str()).copied() else { continue };
            cancel_for(bot, id)?;
            let qty = bot.bank_items().get(&id).copied().unwrap_or(0);
            if qty == 0 {
                continue;
            }
            withdraw_all(bot, &[id])?;
            drop_all(bot, id, start_tick, start_pause, parsed.budget_max)?;
            bot.log(&format!("cleanup D17 dropped {name} x{qty}"));
        }
    }
    let types = bank_types(bot);
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
        assert!(parse_args(&["cleanup".to_string(), "--frobnicate".to_string()]).is_err(), "junk flags fail loud");
    }
}
