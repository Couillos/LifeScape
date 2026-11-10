//! Fishing activity: best spot for the level, banks full loads,
//! sells fish to fund tool and bait upgrades. One load per cycle step:
//! everything it calls lives in the kit (`kit::fish` driver,
//! `kit::spots` pool, `kit::tools` rank).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::{obj_id, obj_price, pack_total};
use crate::kit::shop::{buy_best_affordable, buy_stock, sell_for_target};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Fish banked loads in the open session (0 = forever).
pub fn run_fishing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("fishing", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let side_bank = (bot.tables.com_id)("bank_side:inv")
        .ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    let main_bank = (bot.tables.com_id)("bank_main:inv")
        .ok_or_else(|| ScriptError::Usage("bank_main:inv missing from component table".to_string()))?;
    let side_shop = (bot.tables.com_id)("shop_template_side:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template_side:inv missing from component table".to_string()))?;
    let main_shop = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let _ = (side_bank, main_bank);
    let coins = obj_id(bot, "coins")?;
    let mut level = learn_level(bot, crate::kit::fish::FISH_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut bank_coins = 0u32;
    if crate::kit::tools::FISH_SPEC.best_held(bot).is_none() {
        // Keep the tool family in the pack through the bootstrap trip:
        // `bank_trip` only deposits, it never withdraws, so a
        // coins-only keep banks the very net we are looking for and the
        // run below fails `blocked` on a tool sitting in the bank (seen
        // live, L11). Then pull back a net banked by another skill's
        // trip before buying or blocking (mining bootstrap pattern).
        let mut keep = vec![coins];
        keep.extend(crate::kit::tools::FISH_SPEC.known_ids(bot));
        bank_coins = bank_trip(bot, &bank, &keep, 0)?;
        if crate::kit::tools::FISH_SPEC.best_held(bot).is_none()
            && crate::kit::tools::restore_banked_tool(bot, &bank, &crate::kit::tools::FISH_SPEC)?
        {
            bot.log("ok net restored from bank");
        }
        if crate::kit::tools::FISH_SPEC.best_held(bot).is_none() {
            let net_price = obj_price("net") as u32;
            if pack_total(bot, coins) >= net_price && buy_stock(bot, "net", 1)? > 0 {
                bot.log("ok bootstrapped net");
            } else {
                return Err(crate::kit::tools::blocked_tool("net"));
            }
        }
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let (ok, new_level) = crate::kit::fish::fish_load(step_bot, level, &mut bank, coins, side_shop, main_shop)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Usage(format!("fish failed at {:?}", step_bot.position())));
        }
        let (cands, cheapest) = crate::kit::tools::cheapest_fish_upgrade(step_bot, level);
        if !cands.is_empty() {
            sell_for_target(step_bot, &[], coins, cheapest.max(0) as u32 - bank_coins.min(cheapest.max(0) as u32))?;
        }
        let mut keep: Vec<u16> = crate::kit::tools::FISH_SPEC.known_ids(step_bot).into_iter().collect();
        keep.extend(task_keep.iter().copied());
        bank_coins = bank_trip(step_bot, &bank, &keep, 0)?;
        // Post-bank upgrade: only what the pack affords, best-first with
        // downgrade on shop failure (shared aborts: death/conn/interrupt).
        let _ = buy_best_affordable(step_bot, &cands)?;
        step_bot.log(&format!("ok load {} banked (fishing={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    let _ = bank_coins;
    outcome
}
