//! Woodcut activity: best trees for the level, banks full loads. One
//! load per cycle step: everything it calls lives in the kit
//! (`kit::woodcut` driver, `kit::trees` pool, `kit::tools` rank,
//! `legacy::bootstrap` life decisions until the brain replaces them).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::skill_common::{
    bank_trip, bank_trip_resilient, check_events, learn_level, nearest_bank, obj_id, pack_total,
    sell_for_target,
};
use crate::{ScriptError, ScriptResult};

/// Consecutive axeless loads before the run reports blocked instead of
/// hunting forever: the brain files an obtain task from `missing`.
const AXELESS_LIMIT: u32 = 3;

/// Cut banked loads in the open session (0 = forever).
pub fn run_woodcut(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("woodcut", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    if spec.allow_wild {
        bot.allow_wild = true;
        bot.log("hunter zones allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    crate::skill_common::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let tiers = knowledge::trees::all_tiers();
    let mut level = learn_level(bot, crate::kit::woodcut::WC_STAT, 1);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    let mut bank_coins = 0u32;
    // Without shop buys only free axes count (pack reattach, banked
    // restore): no free axe anywhere blocks now instead of hunting for
    // coins that cannot be spent.
    if !spec.allow_shop {
        if !crate::kit::tools::reclaim_free(bot, &bank, &crate::kit::tools::AXE_SPEC, crate::kit::tools::AXE_HANDLE, crate::kit::tools::AXE_HEAD_SUFFIX)? {
            return Err(crate::kit::tools::blocked_tool("bronze_axe"));
        }
    }
    // Threat bans outlive one load: a killer tree stays banned while its
    // neighbours get cut, instead of repicking the same death every load.
    let mut banned = crate::skill_gather::BanSet::new();
    // Observation memory for the run (regrow ETAs, stump sightings,
    // refusal strikes, gainless visits): owned by the bot, never shared,
    // never persisted — it dies with the run.
    let mut observe = crate::kit::observe::Observe::fresh();
    let mut axeless = 0u32;
    let outcome = run(bot, &spec, |step_bot, state| {
        // Death may have moved us across the map: re-sync the bank cheaply
        // (no walking) so spots and treks start from the real position.
        bank = nearest_bank(step_bot)?;
        check_events(step_bot)?;
        // Death abort inside means the load was lost: re-gear (rebuy the
        // axe when missing) and retry the load, for as long as it takes.
        // Broke is not an error either: no axe means hunt again next
        // iteration (combat xp always progresses). Only conn/interrupt
        // and structural failures (toll, unreachable bank) fail loud.
        // Anything else fails loud as before.
        if spec.allow_shop {
            match crate::legacy::bootstrap::ensure_axe(step_bot, &mut bank, coins, &log_ids) {
                Ok(Some(coins_)) => bank_coins = coins_,
                Ok(None) => {}
                Err(error) if crate::skill_common::is_death_abort(&error) => {
                    step_bot.log("died re-gearing, retrying the load");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
            }
        }
        if crate::kit::tools::AXE_SPEC.best_held(step_bot).is_none() {
            // Without shop buys a lost axe can still come back free
            // (pack reattach, banked restore) before blocking.
            if !spec.allow_shop
                && crate::kit::tools::reclaim_free(step_bot, &bank, &crate::kit::tools::AXE_SPEC, crate::kit::tools::AXE_HANDLE, crate::kit::tools::AXE_HEAD_SUFFIX)?
            {
                axeless = 0;
                return Ok(StepControl::Continue);
            }
            axeless += 1;
            if axeless >= AXELESS_LIMIT {
                return Err(crate::kit::tools::blocked_tool("bronze_axe"));
            }
            step_bot.log("still axeless after re-gear, hunting again");
            return Ok(StepControl::Continue);
        }
        axeless = 0;
        let (ok, new_level) = crate::kit::woodcut::chop_load(step_bot, &tiers, &mut bank, coins, level, &mut banned, &mut observe)?;
        level = new_level;
        // A failed load (threats, stumps, stall) retries with bans kept:
        // stumps regrow and hunters wander off.
        if !ok {
            step_bot.log("chop load failed, retrying with fresh bans");
            return Ok(StepControl::Continue);
        }
        let (cands, cheapest) = crate::kit::tools::AXE_SPEC.cheapest_upgrade(step_bot, |_| true);
        if !cands.is_empty() {
            let have = pack_total(step_bot, coins);
            sell_for_target(
                step_bot,
                &log_ids.iter().copied().collect::<Vec<_>>(),
                coins,
                (cheapest as u32).saturating_sub(bank_coins + have),
            )?;
            if crate::legacy::bootstrap::can_buy_now(pack_total(step_bot, coins), cheapest, &cands) {
                match crate::legacy::bootstrap::buy_axe_upgrade(step_bot, &cands)? {
                    Some(_) => {}
                    None => {
                        step_bot.log(&format!(
                            "upgrade missed: {} too costly or keeper unreachable at {:?}, {} coins banked next trip, never carried to spot",
                            cands.join("+"),
                            step_bot.position(),
                            pack_total(step_bot, coins)
                        ));
                    }
                }
            }
        }
        let mut keep: Vec<u16> = crate::kit::tools::AXE_SPEC.keep_ids(step_bot);
        keep.extend(task_keep.iter().copied());
        // Death on the bank trip loses the load: re-gear next iteration
        // and retry it instead of failing the run. Anything else fails
        // loud as before.
        match bank_trip_resilient(step_bot, &mut bank, &keep, cheapest as u32, &log_ids.iter().copied().collect::<Vec<_>>()) {
            Ok(coins_) => bank_coins = coins_,
            Err(error) if crate::skill_common::is_death_abort(&error) => {
                step_bot.log("died on the bank trip, re-gearing to continue");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        // Post-bank upgrade: only what the pack affords, best-first with
        // bronze fallback inside. No coins means no walk to Bob at all.
        // The shop trip can leave two axes in the pack (old + upgrade):
        // stash then, so the spot sees one axe and no coins.
        if pack_total(step_bot, coins) > 0 {
            let _ = crate::legacy::bootstrap::buy_axe_upgrade(step_bot, &cands)?;
        }
        let axes_held: u32 = crate::kit::tools::AXE_SPEC.known_ids(step_bot).iter().map(|id| pack_total(step_bot, *id)).sum();
        if pack_total(step_bot, coins) > 0 || axes_held > 1 {
            // Failed buys must not ride to the spot: stash now or abort.
            let mut stash_keep: Vec<u16> = crate::kit::tools::AXE_SPEC.keep_ids(step_bot);
            stash_keep.extend(task_keep.iter().copied());
            bank_trip(step_bot, &bank, &stash_keep, 0)?;
            if pack_total(step_bot, coins) > 0 {
                return Err(ScriptError::Usage(format!(
                    "coins left after banking at {:?}, refusing to carry to spot",
                    step_bot.position()
                )));
            }
        }
        step_bot.log(&format!("ok load {} banked (wc={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    let _ = bank_coins;
    outcome
}
