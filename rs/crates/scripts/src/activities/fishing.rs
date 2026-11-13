//! Fishing activity: the brain's fish (`fishing raw_trout --site ...`)
//! with its own method, banked full loads. A thin executor: the ordered
//! fish's tool and bait come from the pack or the bank, gaps block by
//! exact name (`fly_fishing_rod`, `feather`), never a shop buy; manual
//! runs without a fish take the best geared tier by level. Everything
//! it calls lives in the kit (`kit::fish` driver, `kit::fish_gear`,
//! `kit::spots` pool).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Fish banked loads in the open session (0 = forever).
pub fn run_fishing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("fishing", args)?;
    crate::kit::toll::arm(bot, spec.allow_paid)?;
    bot.allow_shop = spec.allow_shop;
    bot.assigned_site = spec.site;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let coins = obj_id(bot, "coins")?;
    let mut level = learn_level(bot, crate::kit::fish::FISH_STAT, 1);
    let target = match spec.resource.first() {
        Some(fish) => Some(crate::kit::fish_gear::ordered_tier(fish, level)?),
        None => None,
    };
    let mut bank = match crate::kit::site::site_bank(bot) {
        Some(bank) => bank,
        None => nearest_bank(bot)?,
    };
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    // Gear before anything: the ordered method's exact gaps (pack and
    // known bank counted), or on manual runs any geared tier.
    match target.as_ref() {
        Some(tier) => {
            let missing = crate::kit::fish_gear::gear_missing(bot, tier);
            if !missing.is_empty() {
                return Err(ScriptError::Blocked { missing });
            }
        }
        None => {
            crate::kit::fish_gear::select_fish_tier(bot, level)?;
        }
    }
    // Picker guard: no water in range is structural (spots don't respawn
    // like fish), so fail fast with the reason instead of burning dead
    // repicks in a dry pit while the brain re-assigns fishing in a loop.
    if crate::kit::fish::structurally_dry(bot, level, &bank, target.as_ref()) {
        return Err(crate::kit::fish::dry_water_error(bot));
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let (ok, new_level) = crate::kit::fish::fish_load(step_bot, level, &mut bank, coins, target.as_ref())?;
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
        // Gear (tools and bait) stays in the pack across bank trips.
        let mut keep: Vec<u16> = crate::kit::tools::FISH_SPEC.known_ids(step_bot).into_iter().collect();
        keep.extend(["feather", "fishing_bait"].iter().filter_map(|bait| obj_id(step_bot, bait).ok()));
        keep.extend(task_keep.iter().copied());
        bank_trip(step_bot, &bank, &keep, 0)?;
        step_bot.log(&format!("ok load {} banked (fishing={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
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
