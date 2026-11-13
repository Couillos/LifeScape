//! Fishing activity: best spot for the level, banks full loads,
//! sells fish to fund tool and bait upgrades. One load per cycle step:
//! everything it calls lives in the kit (`kit::fish` driver,
//! `kit::spots` pool, `kit::tools` rank).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::{obj_id, pack_total, shop_price};
use crate::kit::shop::{buy_best_affordable, buy_stock};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Fish banked loads in the open session (0 = forever).
pub fn run_fishing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("fishing", args)?;
    crate::kit::toll::arm(bot, spec.allow_paid)?;
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
            let net_price = shop_price("net").unwrap_or(i64::MAX).min(i64::from(u32::MAX)) as u32;
            if pack_total(bot, coins) >= net_price && buy_stock(bot, "net", 1)? > 0 {
                bot.log("ok bootstrapped net");
            } else {
                return Err(crate::kit::tools::blocked_tool("net"));
            }
        }
    }
    // Picker guard: no water in range is structural (spots don't respawn
    // like fish), so fail fast with the reason instead of burning dead
    // repicks in a dry pit while the brain re-assigns fishing in a loop.
    if crate::kit::fish::structurally_dry(bot, level, &bank) {
        return Err(crate::kit::fish::dry_water_error(bot));
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let (ok, new_level) = crate::kit::fish::fish_load(step_bot, level, &mut bank, coins, side_shop, main_shop)?;
        level = new_level;
        if !ok {
            return Err(ScriptError::Failed(format!("fish failed at {:?}", step_bot.position())));
        }
        // Training a level (`--no-bank`): only XP counts, the catch is
        // dropped where it was caught. No bank loop at all (the Draynor
        // bank sits inside the dark wizards' aggro square: bots died on
        // every trip between it and the water).
        if spec.no_bank {
            let dropped = drop_catch(step_bot, &task_keep)?;
            step_bot.log(&format!("ok load {} dropped ({dropped} fish, fishing={level})", state.units() + 1));
            return Ok(StepControl::UnitDone);
        }
        // PRD3: the catch is never sold to a shop; upgrades are funded
        // from banked coins (market sales).
        let (cands, _cheapest) = crate::kit::tools::cheapest_fish_upgrade(step_bot, level);
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

/// Drop every raw fish in the pack (any tier's product), except names
/// the brain asked to keep. Returns how many stacks went.
fn drop_catch(bot: &mut Bot, keep: &[u16]) -> Result<u32, ScriptError> {
    let fish: Vec<u16> = lifescape_gamedata::fishing::all_fish_tiers()
        .iter()
        .flat_map(|tier| tier.products.iter().filter_map(|product| obj_id(bot, product).ok()))
        .filter(|id| !keep.contains(id))
        .collect();
    let mut dropped = 0u32;
    for id in fish {
        while pack_total(bot, id) > 0 {
            if !bot.drop(id, 20)? {
                return Err(ScriptError::Failed(format!("drop of obj {id} never landed at {:?}", bot.position())));
            }
            dropped += 1;
        }
    }
    Ok(dropped)
}
