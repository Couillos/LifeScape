//! Mining activity: best F2P rock for the level, banks full loads.
//! One load per cycle step: everything it calls lives in the kit
//! (`kit::mine` driver, `kit::spots` pool, `kit::tools` rank).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::{obj_id, obj_price, pack_ids, pack_total};
use crate::kit::shop::{buy_best_affordable, buy_stock};
use crate::kit::survival::{check_events, learn_level};
use crate::{ScriptError, ScriptResult};

/// Mine banked loads in the open session (0 = forever).
pub fn run_mining(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("mining", args)?;
    crate::kit::toll::arm(bot, spec.allow_paid)?;
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    crate::kit::survival::wait_pack_streamed(bot)?;
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
            let need_cross = crate::kit::bank::toll_gate_x(bot)
            .is_some_and(|line| (bank.x as i32 >= line) != (bot.world.x >= line));
            let target = bronze_price + if need_cross { 10 } else { 0 };
            if pack_total(bot, coins) < target {
                bank_coins = bank_trip(bot, &bank, &keep, u32::MAX / 2)?;
            }
            if need_cross && pack_total(bot, coins) >= target && !crate::kit::bank::cross_toll_gate(bot)? {
                return Err(ScriptError::Failed(format!("cannot cross toll gate at {:?}", bot.position())));
            }
            if pack_total(bot, coins) >= bronze_price && buy_stock(bot, "bronze_pickaxe", 1)? > 0 {
                bot.log("ok bootstrapped bronze pickaxe");
            } else {
                return Err(crate::kit::tools::blocked_tool("bronze_pickaxe"));
            }
        }
    }
    let mut toolless = 0u32;
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let (ok, new_level) = crate::kit::mine::mine_load(step_bot, &tiers, &mut bank, coins, level)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Failed(format!("mine failed at {:?}", step_bot.position())));
        }
        // PRD3: ore is never sold to a shop; an upgrade is funded from
        // banked coins (market sales), never from a general-store dump.
        let (cands, cheapest) = crate::kit::tools::cheapest_pickaxe_upgrade(step_bot, level);
        let mut keep: Vec<u16> = crate::kit::tools::PICK_SPEC.known_ids(step_bot).into_iter().collect();
        keep.extend(task_keep.iter().copied());
        bank_coins = bank_trip(step_bot, &bank, &keep, cheapest as u32)?;
        // Post-bank upgrade: only what the pack affords, best-first with
        // downgrade on shop failure (shared aborts: death/conn/interrupt).
        let _ = buy_best_affordable(step_bot, &cands)?;
        toolless = if crate::kit::tools::PICK_SPEC.best_held(step_bot).is_none() { toolless + 1 } else { 0 };
        if toolless >= 2 {
            return Err(crate::kit::tools::blocked_tool("bronze_pickaxe"));
        }
        step_bot.log(&format!("ok load {} banked (mining={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    let _ = bank_coins;
    outcome
}
