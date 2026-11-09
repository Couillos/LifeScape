//! Mining activity: best F2P rock for the level, banks full loads.
//! The loads loop only: everything it calls lives in the kit
//! (`kit::mine` driver, `kit::spots` pool, `kit::tools` rank).

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, buy_stock, check_events, learn_level, nearest_bank, obj_id, obj_price, pack_ids,
    pack_total, sell_for_target,
};
use crate::skill_trade::buy_best_affordable;
use crate::{ScriptError, ScriptResult};

/// Mine banked loads in the open session (0 = forever).
pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("mining", args)?;
    let loads = spec.count;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = crate::kit::spots::usable_rock_tiers();
    let mut level = learn_level(bot, crate::kit::mine::MINING_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut bank_coins = 0u32;
    if crate::kit::tools::PICK_SPEC.best_held(bot).is_none() || pack_ids(bot).iter().any(|id| !crate::kit::tools::PICK_SPEC.known_ids(bot).contains(id) && *id != coins) {
        if crate::kit::tools::reattach_tool(bot, crate::kit::tools::PICK_HANDLE, crate::kit::tools::PICK_HEAD_SUFFIX, &crate::kit::tools::PICK_SPEC)? {
            bot.log("ok pickaxe reattached from pack, no bootstrap needed");
        }
        let keep: Vec<u16> = crate::kit::tools::PICK_SPEC.known_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if crate::kit::tools::PICK_SPEC.best_held(bot).is_none() && crate::kit::tools::restore_from_bank(bot, &bank, &crate::kit::tools::PICK_SPEC, crate::kit::tools::PICK_HANDLE, crate::kit::tools::PICK_HEAD_SUFFIX)? {
            bot.log("ok pickaxe restored from bank");
        }
        if crate::kit::tools::PICK_SPEC.best_held(bot).is_none() {
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
        let (ok, new_level) = crate::kit::mine::mine_load(bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("mine failed at {:?}", bot.position())));
        }
        let (cands, cheapest) = crate::kit::tools::cheapest_pickaxe_upgrade(bot, level);
        if !cands.is_empty() {
            sell_for_target(bot, &ore_ids_for(bot, &tiers), coins, cheapest as u32)?;
        }
        let keep: Vec<u16> = crate::kit::tools::PICK_SPEC.known_ids(bot).into_iter().collect();
        bank_coins = bank_trip(bot, &bank, &keep, cheapest as u32)?;
        // Post-bank upgrade: only what the pack affords, best-first with
        // downgrade on shop failure (shared aborts: death/conn/interrupt).
        let _ = buy_best_affordable(bot, &cands)?;
        toolless = if crate::kit::tools::PICK_SPEC.best_held(bot).is_none() { toolless + 1 } else { 0 };
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
