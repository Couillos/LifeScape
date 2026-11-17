//! Fishing activity: the brain's fish (`fishing raw_trout --site ...`)
//! with its own method, banked full loads. A thin executor: the ordered
//! fish's tool and bait come from the pack or the bank, gaps block by
//! exact name (`fly_fishing_rod`, `feather`), never a shop buy; manual
//! runs without a fish take the best geared tier by level. Everything
//! it calls lives in the kit (`kit::fish` driver, `kit::fish_gear`,
//! `kit::spots` pool).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run};
use crate::kit::bank::{bank_trip, nearest_bank};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

/// Fish banked loads in the open session (0 = forever).
pub fn run_fishing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("fishing", args)?;
    crate::kit::toll::arm_fares(bot, spec.allow_paid, spec.allow_boat, spec.site)?;
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
    // known bank counted), or on manual runs the best geared tier.
    let tier = match target.as_ref() {
        Some(tier) => {
            let missing = crate::kit::fish_gear::gear_missing(bot, tier);
            if !missing.is_empty() {
                return Err(ScriptError::Blocked { missing });
            }
            tier.clone()
        }
        None => crate::kit::fish_gear::select_fish_tier(bot, level)?,
    };
    depart(bot, &tier, &bank)?;
    // Picker guard: no water in range is structural (spots don't respawn
    // like fish), so fail fast with the reason instead of burning dead
    // repicks in a dry pit while the brain re-assigns fishing in a loop.
    if crate::kit::fish::structurally_dry(bot, level, &bank, target.as_ref()) {
        return Err(crate::kit::fish::dry_water_error(bot));
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        let until = state.want_goal(step_bot, &spec);
        let (ok, new_level) = crate::kit::fish::fish_load(step_bot, level, &mut bank, coins, target.as_ref(), until)?;
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
            return Ok(state.load_done(step_bot, &format!("dropped ({dropped} fish, fishing={level})")));
        }
        // Gear (tools and bait) stays in the pack across bank trips.
        let mut keep: Vec<u16> = crate::kit::tools::FISH_SPEC.known_ids(step_bot).into_iter().collect();
        keep.extend(["feather", "fishing_bait"].iter().filter_map(|bait| obj_id(step_bot, bait).ok()));
        keep.extend(task_keep.iter().copied());
        bank_trip(step_bot, &bank, &keep, 0)?;
        Ok(state.load_done(step_bot, &format!("banked (fishing={level})")))
    })
}

/// Leave for the brain's site (Karamja by boat, far rivers): water is
/// judged where the bot was sent, never around the departure dock. The
/// tool and bait come out of the bank BEFORE the trip (the travel coins
/// were carried by `arm_fares`): a banked lobster pot found missing on
/// the Musa Point pier cost a boat round trip and its 60 coins.
fn depart(bot: &mut Bot, tier: &lifescape_gamedata::fishing::FishTier, bank: &lifescape_gamedata::banks::Bank) -> Result<(), ScriptError> {
    crate::kit::fish_gear::gear_before_departure(bot, tier, bank)?;
    crate::kit::site::reach_site(bot)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    /// Live admin 2026-11-17 (`farm:raw_lobster`): at the Draynor bank
    /// with the pot banked, the run sailed to Karamja first. The bank
    /// comes first: offline the booth never opens, so the departure stops
    /// on the pot (`Blocked`), never on the trip to the site.
    #[test]
    fn a_banked_tool_is_taken_before_the_trip() {
        let mut bot = crate::kit::fish_gear::tests::gear_bot(&[("coins", 60)], &[("lobster_pot", 1)]);
        bot.world.level = 0;
        bot.world.x = 3092;
        bot.world.z = 3243;
        bot.allow_paid = true;
        bot.assigned_site = Some((2925, 3181, 0));
        let bank = crate::kit::site::site_bank(&bot).expect("Draynor serves Karamja");
        let tier = crate::kit::fish_gear::ordered_tier("raw_lobster", 40).expect("lobster at 40");
        let error = super::depart(&mut bot, &tier, &bank).expect_err("offline: nothing leaves");
        match error {
            crate::ScriptError::Blocked { missing } => assert_eq!(missing, vec!["lobster_pot".to_string()]),
            other => panic!("the pot is fetched before the trip, got: {other}"),
        }
    }
}
