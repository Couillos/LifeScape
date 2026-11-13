//! Cook activity: the produce declaration for food (shrimp first), run
//! through the shared brick. The recipe fixes the level, the XP and the
//! burnt byproduct (`Recipe.burnt`); the only particularités are the
//! station order (a range burns less than a fire) and the pack-to-bank
//! loop. No life decisions: no buying, no selling, no spot switch.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const COOK_STAT: u8 = 7;
pub const DEFAULT_RAW: &str = "raw_shrimp";

/// Stations in preference order: ranges first (they burn less), then
/// fireplaces, then open fires (player-made, live only).
pub const STATIONS: [&str; 6] =
    ["range", "fireplace", "grandfireplace", "grandfireplace_oldbrick", "fireplacecookingpot", "fire"];

const COOK: Produce<'static> =
    Produce { verb: "cooking", site: ProduceSite::Loc(&STATIONS), ui: ProduceUi::Direct };

/// A priori yield of one shrimp load cycle (bank, walk, cook, bank),
/// replaced once three measured runs land. Shrimp: 30 xp each.
pub const PRIOR_DISHES_PER_6000: u64 = 400;
pub const PRIOR_XP_PER_6000: u64 = 12_000;

/// Cook banked loads in the open session: `cook [raw] [count]`
/// (`raw_shrimp` when omitted, 0 = forever).
pub fn run_cook(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("cook", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let raw_name = spec.resource.first().map(String::as_str).unwrap_or(DEFAULT_RAW);
    let recipe = produce::cook_recipe(raw_name)
        .ok_or_else(|| ScriptError::Usage(format!("no F2P cook recipe eats {raw_name:?} at {:?}", bot.position())))?;
    let level = learn_level(bot, COOK_STAT, 1);
    if level < recipe.level {
        return Err(crate::kit::cycle::blocked_level("cooking", i64::from(recipe.level)));
    }
    let raw = obj_id(bot, raw_name)?;
    let mut products: Vec<u16> = recipe.outputs.iter().filter_map(|(name, _)| obj_id(bot, name).ok()).collect();
    if let Some(burnt) = recipe.burnt.as_deref() {
        if let Ok(id) = obj_id(bot, burnt) {
            products.push(id);
        }
    }
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_raw: Vec<u16> = vec![raw];
    keep_raw.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_raw, 0)?;
    fill_raw(bot, &bank, raw)?;
    if pack_total(bot, raw) == 0 {
        return Err(ScriptError::Blocked { missing: vec![raw_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, raw) == 0 {
            bank_trip(step_bot, &bank, &keep_raw, 0)?;
            fill_raw(step_bot, &bank, raw)?;
            if pack_total(step_bot, raw) == 0 {
                return Err(ScriptError::Blocked { missing: vec![raw_name.to_string()] });
            }
        }
        let job = ProduceJob {
            spec: &COOK,
            input: raw,
            input_name: raw_name,
            want: 0,
            products: products.clone(),
            near: (bank.x as i32, bank.z as i32),
        };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died cooking, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => {
                step_bot.log(&format!("ok cooked {} ({} made)", batch.consumed, batch.made));
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} cooked (cooking={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Top the pack up with raw from the bank (nothing when the bank holds
/// none or the booth is far: the caller reports `blocked`, never an
/// empty walk).
fn fill_raw(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, raw: u16) -> Result<(), ScriptError> {
    let free = crate::kit::pack::free_slots(bot) as u32;
    if free == 0 {
        return Ok(());
    }
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &[raw])? {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[raw], free)?;
    Ok(())
}
