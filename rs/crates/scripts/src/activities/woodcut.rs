//! Woodcut activity: best trees for the level, banks full loads. One
//! load per cycle step: everything it calls lives in the kit
//! (`kit::woodcut` driver, `kit::trees` pool, `kit::tools` rank).
//! Autonomous (D24 done): no life decisions remain — no selling, no axe
//! shopping, no goblin hunts. Broke or axeless blocks with
//! `blocked{missing: bronze_axe}` and the brain files the obtain task.

#![forbid(unsafe_code)]

use std::collections::HashSet;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip_resilient, nearest_bank};
use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, learn_level};
use crate::ScriptResult;

/// Consecutive axeless loads before the run reports blocked: the brain
/// files an obtain task from `missing`, no hunting here.
const AXELESS_LIMIT: u32 = 3;

/// Tree tiers the run may cut: the brain's log alone (`woodcut
/// willow_logs`), or every F2P tier on a manual run. Unknown logs are a
/// Usage error, a level gap `blocked{level:woodcutting:N}`.
pub fn ordered_tree_tiers(resource: Option<&str>, level: i64) -> Result<Vec<knowledge::trees::TreeTier>, crate::ScriptError> {
    let tiers = knowledge::trees::all_tiers();
    let Some(log) = resource else { return Ok(tiers) };
    let ordered: Vec<knowledge::trees::TreeTier> = tiers.into_iter().filter(|tier| tier.product.eq_ignore_ascii_case(log) && tier.product_members != Some(true)).collect();
    let Some(need) = ordered.iter().map(|tier| tier.level).min() else {
        return Err(crate::ScriptError::Usage(format!("no F2P tree yields {log:?}")));
    };
    if level < need {
        return Err(crate::kit::cycle::blocked_level("woodcutting", need));
    }
    Ok(ordered)
}

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
    crate::kit::survival::wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let mut level = learn_level(bot, crate::kit::woodcut::WC_STAT, 1);
    let tiers = ordered_tree_tiers(spec.resource.first().map(String::as_str), level)?;
    bot.assigned_site = spec.site;
    let mut bank = match crate::kit::site::site_bank(bot) {
        Some(bank) => bank,
        None => nearest_bank(bot)?,
    };
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let log_ids: HashSet<u16> = tiers.iter().filter_map(|tier| obj_id(bot, &tier.product).ok()).collect();
    // Free axes only (pack reattach, banked restore): the shop cannot
    // conjure one, and buying upgrades is the brain's obtain task now.
    if !crate::kit::tools::reclaim_free(bot, &bank, &crate::kit::tools::AXE_SPEC, crate::kit::tools::AXE_HANDLE, crate::kit::tools::AXE_HEAD_SUFFIX)? {
        return Err(crate::kit::tools::blocked_tool("bronze_axe"));
    }
    // Threat bans outlive one load: a killer tree stays banned while its
    // neighbours get cut, instead of repicking the same death every load.
    let mut banned = crate::kit::bans::BanSet::new();
    // Observation memory for the run (regrow ETAs, stump sightings,
    // refusal strikes, gainless visits): owned by the bot, never shared,
    // never persisted — it dies with the run.
    let mut observe = crate::kit::observe::Observe::fresh();
    let mut axeless = 0u32;
    run(bot, &spec, |step_bot, state| {
        // Death may have moved us across the map: re-sync the bank cheaply
        // (no walking) so spots and treks start from the real position.
        bank = match crate::kit::site::site_bank(step_bot) {
            Some(site_bank) => site_bank,
            None => nearest_bank(step_bot)?,
        };
        check_events(step_bot)?;
        if crate::kit::tools::AXE_SPEC.best_held(step_bot).is_none() {
            // A lost axe can still come back free (pack reattach, banked
            // restore) before blocking.
            if crate::kit::tools::reclaim_free(step_bot, &bank, &crate::kit::tools::AXE_SPEC, crate::kit::tools::AXE_HANDLE, crate::kit::tools::AXE_HEAD_SUFFIX)? {
                axeless = 0;
                return Ok(StepControl::Continue);
            }
            axeless += 1;
            if axeless >= AXELESS_LIMIT {
                return Err(crate::kit::tools::blocked_tool("bronze_axe"));
            }
            step_bot.log("still axeless, retrying re-gear next load");
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
        let mut keep: Vec<u16> = crate::kit::tools::AXE_SPEC.keep_ids(step_bot);
        keep.extend(task_keep.iter().copied());
        // Death on the bank trip loses the load: re-gear next iteration
        // and retry it instead of failing the run. Anything else fails
        // loud as before.
        match bank_trip_resilient(step_bot, &mut bank, &keep, 0, &log_ids.iter().copied().collect::<Vec<_>>()) {
            Ok(_) => {}
            Err(error) if crate::kit::survival::is_death_abort(&error) => {
                step_bot.log("died on the bank trip, re-gearing to continue");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
        }
        step_bot.log(&format!("ok load {} banked (wc={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

#[cfg(test)]
mod tests {
    use super::AXELESS_LIMIT;

    #[test]
    fn axeless_blocks_fast() {
        assert!(AXELESS_LIMIT <= 5, "no axe-hunting loops: {AXELESS_LIMIT} retries then blocked");
    }

    /// The brain's log is the only tier cut; above the level it blocks.
    #[test]
    fn ordered_log_only_and_level_gap_blocks() {
        let willow = super::ordered_tree_tiers(Some("willow_logs"), 40).expect("willow at 40");
        assert!(!willow.is_empty() && willow.iter().all(|tier| tier.product == "willow_logs"));
        match super::ordered_tree_tiers(Some("yew_logs"), 40) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["level:woodcutting:60".to_string()]),
            other => panic!("yew at 40 must block on level, got {other:?}"),
        }
        assert!(matches!(super::ordered_tree_tiers(Some("pebble"), 99), Err(crate::ScriptError::Usage(_))));
    }
}
