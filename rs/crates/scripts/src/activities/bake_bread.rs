//! Bake bread: flour plus water into dough (inventory choice), dough on
//! a range (catalogue §5.4, P2). The flour comes from the mill chain
//! (P3.1); this script banks loads, mixes the dough and bakes it, like
//! `cook` with a prep phase. Serveur : dough.rs2 (choix pain), bake
//! `bread_dough -> bread` L1 400 XP (`cooking_generic`).
//!
//! Process: poste cuisinière (32, feux 20) · action cuire · interface
//! directe · intrants pâte à pain · produit pain. Les clics inventaire
//! viennent de `cook_meals` (même famille, pas de second code).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use super::cook_meals::pour;
use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const COOK_STAT: u8 = 7;
pub const FLOUR: &str = "pot_flour";
pub const WATER: &str = "jug_water";
pub const DOUGH: &str = "bread_dough";
pub const BREAD_LEVEL: i64 = 1;

/// A priori yield of one flour load cycle, replaced by measured rates.
pub const PRIOR_LOAVES_PER_6000: u64 = 150;
pub const PRIOR_XP_PER_6000: u64 = 6_000;

const BAKE: Produce<'static> =
    Produce { verb: "baking", site: ProduceSite::Loc(&super::cook::STATIONS), ui: ProduceUi::Direct };

/// Bake banked loads in the open session: `bake_bread [count]`.
pub fn run_bake_bread(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("bake_bread", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, COOK_STAT, 1);
    if level < BREAD_LEVEL {
        return Err(crate::kit::cycle::blocked_level("cooking", i64::from(BREAD_LEVEL)));
    }
    let recipe = produce::cook_recipe(DOUGH)
        .ok_or_else(|| ScriptError::Usage(format!("no F2P cook recipe bakes {DOUGH:?} at {:?}", bot.position())))?;
    let (flour, water, dough) = (obj_id(bot, FLOUR)?, obj_id(bot, WATER)?, obj_id(bot, DOUGH)?);
    let mut products: Vec<u16> = recipe.outputs.iter().filter_map(|(name, _)| obj_id(bot, name).ok()).collect();
    if let Some(burnt) = recipe.burnt.as_deref() {
        if let Ok(id) = obj_id(bot, burnt) {
            products.push(id);
        }
    }
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_dough: Vec<u16> = vec![flour, water, dough];
    keep_dough.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_dough, 0)?;
    fill_bread(bot, &bank, flour, water)?;
    if pack_total(bot, flour) == 0 || pack_total(bot, water) == 0 {
        return Err(ScriptError::Blocked { missing: vec![FLOUR.to_string(), WATER.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, dough) == 0 {
            if pack_total(step_bot, flour) == 0 || pack_total(step_bot, water) == 0 {
                bank_trip(step_bot, &bank, &keep_dough, 0)?;
                fill_bread(step_bot, &bank, flour, water)?;
            }
            pour(step_bot, &bank, &keep_dough, FLOUR, WATER, Some("Bread dough"))?;
            if pack_total(step_bot, dough) == 0 {
                return Err(ScriptError::Blocked { missing: vec![FLOUR.to_string(), WATER.to_string()] });
            }
        }
        let job = ProduceJob {
            spec: &BAKE,
            input: dough,
            input_name: DOUGH,
            want: 0,
            products: products.clone(),
            near: (bank.x as i32, bank.z as i32),
        };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died baking, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => step_bot.log(&format!("ok baked {} ({} made)", batch.consumed, batch.made)),
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} baked (cooking={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Top the pack with flour and water, one slot left free (the dough
/// choice refuses on a full pack).
fn fill_bread(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, flour: u16, water: u16) -> Result<(), ScriptError> {
    let free = free_slots(bot).saturating_sub(1) as u32;
    if free >= 2 {
        bank_withdraw_items(bot, bank, &[flour], free / 2 + free % 2)?;
        bank_withdraw_items(bot, bank, &[water], free / 2)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BREAD_LEVEL, DOUGH, FLOUR, PRIOR_LOAVES_PER_6000, PRIOR_XP_PER_6000, WATER};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::cook_recipe;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.4: bread dough bakes at level 1 for 400 XP, F2P.
    #[test]
    fn bread_dough_bakes_from_gamedata() {
        let recipe = cook_recipe(DOUGH).expect("dough row resolves");
        assert_eq!(recipe.kind, "cook");
        assert_eq!((recipe.level, recipe.xp), (BREAD_LEVEL, 400));
        assert!(recipe.outputs.iter().any(|(name, _)| name == "bread"), "bread out, got {:?}", recipe.outputs);
        assert_eq!(recipe.burnt.as_deref(), Some("burnt_bread"));
        assert_eq!(recipe.members, Some(false), "F2P row");
        assert_eq!((PRIOR_LOAVES_PER_6000, PRIOR_XP_PER_6000), (150, 6_000));
    }

    /// Flour, water, dough, bread and the returned empties resolve; the
    /// delegated stations start with a range (burns least, like cook).
    #[test]
    fn flour_water_and_stations_resolve() {
        let bot = offline_bot();
        for name in [FLOUR, WATER, DOUGH, "bread", "pot_empty", "jug_empty", "burnt_bread"] {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
        assert_eq!(super::super::cook::STATIONS[0], "range");
    }
}
