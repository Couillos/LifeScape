//! Woodcut activity: best trees for the level, banks full loads.
//! The loads loop only: everything it calls lives in the kit
//! (`kit::woodcut` driver, `kit::trees` pool, `kit::tools` rank,
//! `legacy::bootstrap` life decisions until the brain replaces them).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::skill_common::{
    bank_trip, bank_trip_resilient, check_events, learn_level, nearest_bank, obj_id, pack_total,
    sell_for_target,
};
use crate::{ScriptError, ScriptResult};

/// Cut banked loads in the open session (0 = forever).
pub fn run_woodcut(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("woodcut", args)?;
    let loads = spec.count;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    if spec.allow_wild {
        bot.allow_wild = true;
        bot.log("hunter zones allowed for this run");
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = knowledge::trees::all_tiers();
    let mut level = learn_level(bot, crate::kit::woodcut::WC_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut bank_coins = 0u32;
    let mut done = 0u32;
    // Threat bans outlive one load: a killer tree stays banned while its
    // neighbours get cut, instead of repicking the same death every load.
    let mut banned = crate::skill_gather::BanSet::new();
    // Observation memory for the run (regrow ETAs, stump sightings,
    // refusal strikes, gainless visits): owned by the bot, never shared,
    // never persisted — it dies with the run.
    let mut observe = crate::kit::observe::Observe::fresh();
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
        match crate::legacy::bootstrap::ensure_axe(bot, &mut bank, coins, &log_ids) {
            Ok(Some(coins_)) => bank_coins = coins_,
            Ok(None) => {}
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                bot.log("died re-gearing, retrying the load");
                continue;
            }
            Err(error) => return Err(error),
        }
        if crate::kit::tools::AXE_SPEC.best_held(bot).is_none() {
            bot.log("still axeless after re-gear, hunting again");
            continue;
        }
        let (ok, new_level) = crate::kit::woodcut::chop_load(bot, &tiers, &mut bank, coins, level, &mut banned, &mut observe)?;
        level = new_level;
        // A failed load (threats, stumps, stall) retries with bans kept:
        // stumps regrow and hunters wander off.
        if !ok {
            bot.log("chop load failed, retrying with fresh bans");
            continue;
        }
        let (cands, cheapest) = crate::kit::tools::AXE_SPEC.cheapest_upgrade(bot, |_| true);
        if !cands.is_empty() {
            let have = pack_total(bot, coins);
            sell_for_target(
                bot,
                &log_ids.iter().copied().collect::<Vec<_>>(),
                coins,
                (cheapest as u32).saturating_sub(bank_coins + have),
            )?;
            if crate::legacy::bootstrap::can_buy_now(pack_total(bot, coins), cheapest, &cands) {
                match crate::legacy::bootstrap::buy_axe_upgrade(bot, &cands)? {
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
        let keep: Vec<u16> = crate::kit::tools::AXE_SPEC.keep_ids(bot);
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
            let _ = crate::legacy::bootstrap::buy_axe_upgrade(bot, &cands)?;
        }
        let axes_held: u32 = crate::kit::tools::AXE_SPEC.known_ids(bot).iter().map(|id| pack_total(bot, *id)).sum();
        if pack_total(bot, coins) > 0 || axes_held > 1 {
            // Failed buys must not ride to the spot: stash now or abort.
            let keep_now: Vec<u16> = crate::kit::tools::AXE_SPEC.keep_ids(bot);
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
