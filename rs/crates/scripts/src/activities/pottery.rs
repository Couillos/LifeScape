//! Pottery: clay and water into soft clay (inventory), shaped on a
//! wheel then fired in an oven (catalogue §5.6, P2). T covers the pot
//! only (level 1 both steps, 63 XP each): the wheel choice is the
//! `skill_multi3` grid (`makex_a` = pot, all carried), firing is Direct
//! (one unfired per click, cracks lose the pot). Dishes and bowls reuse
//! the same stations with `makex_b`/`makex_c` later.
//!
//! Process: poste tour (6) + four (2) · action façonner/cuire · interface
//! grille/compteur + directe · intrants argile + eau · produits cru + pot.

#![forbid(unsafe_code)]

use lifescape_gamedata::extra_recipes::ExtraRecipe;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const CLAY: &str = "clay";
pub const WATER: &str = "jug_water";
pub const SOFT: &str = "softclay";
pub const UNFIRED: &str = "pot_unfired";
pub const PRODUCT: &str = "pot_empty";
pub const WHEELS: [&str; 1] = ["potterywheel"];
pub const OVENS: [&str; 1] = ["potteryoven"];

const SHAPE: Produce<'static> = Produce { verb: "shaping", site: ProduceSite::Loc(&WHEELS), ui: ProduceUi::Grid { button: "skill_multi3:makex_a" } };
const FIRE: Produce<'static> = Produce { verb: "firing", site: ProduceSite::Loc(&OVENS), ui: ProduceUi::Direct };

/// Extra row whose outputs contain `name` for this `kind`.
fn extra_row(kind: &str, name: &str) -> Option<ExtraRecipe> {
    lifescape_gamedata::extra_recipes::by_kind(kind).into_iter().find(|entry| entry.outputs.iter().any(|(output, _)| output == name))
}

/// Run one produce job, restocking on death (`Ok(true)` means retried).
fn work_job(bot: &mut Bot, job: &ProduceJob<'_>, what: &str) -> Result<bool, ScriptError> {
    match produce::run_job(bot, job) {
        Err(error) if is_death_abort(&error) => {
            bot.log(&format!("died {what}, restocking to retry"));
            Ok(true)
        }
        Err(error) => Err(error),
        Ok(_) => Ok(false),
    }
}

/// Fire banked loads in the open session: `pottery [count]` (pot only,
/// 0 = forever). Clay and water ride the same load; emptied jugs bank.
pub fn run_pottery(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let spec = crate::kit::cycle::parse_cli("pottery", args)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let shape = extra_row("pottery_shape", UNFIRED).expect("shape row placed");
    let fire = extra_row("pottery_fire", PRODUCT).expect("fire row placed");
    if level < shape.level || level < fire.level {
        return Err(ScriptError::Usage(format!("pottery needs level {} at {:?}", shape.level.max(fire.level), bot.position())));
    }
    let (clay, water, soft, unfired) = (obj_id(bot, CLAY)?, obj_id(bot, WATER)?, obj_id(bot, SOFT)?, obj_id(bot, UNFIRED)?);
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_clay: Vec<u16> = vec![clay, water];
    keep_clay.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_clay, 0)?;
    fill_clay(bot, &bank, clay, water)?;
    if pack_total(bot, clay) == 0 {
        return Err(ScriptError::Blocked { missing: vec![CLAY.to_string()] });
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, clay) == 0 && pack_total(step_bot, soft) == 0 && pack_total(step_bot, unfired) == 0 {
            bank_trip(step_bot, &bank, &keep_clay, 0)?;
            fill_clay(step_bot, &bank, clay, water)?;
            if pack_total(step_bot, clay) == 0 {
                return Err(ScriptError::Blocked { missing: vec![CLAY.to_string()] });
            }
        }
        mix_all(step_bot, clay, water, soft)?;
        let near = (bank.x as i32, bank.z as i32);
        if pack_total(step_bot, soft) > 0 {
            let job = ProduceJob { spec: &SHAPE, input: soft, input_name: SOFT, want: 0, products: vec![unfired], near };
            if work_job(step_bot, &job, "shaping")? { return Ok(StepControl::Continue); }
        }
        if pack_total(step_bot, unfired) > 0 {
            let job = ProduceJob { spec: &FIRE, input: unfired, input_name: UNFIRED, want: 0, products: vec![obj_id(step_bot, PRODUCT)?], near };
            if work_job(step_bot, &job, "firing")? { return Ok(StepControl::Continue); }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} potted (crafting={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

/// Mix every carried clay with water (direct inventory click, no modal:
/// any interface is loud, like stringing).
fn mix_all(bot: &mut Bot, clay: u16, water: u16, soft: u16) -> Result<(), ScriptError> {
    while pack_total(bot, clay) > 0 && pack_total(bot, water) > 0 {
        let had = pack_total(bot, soft);
        let (com, slot) = inv_slot(bot, clay)?;
        let (use_com, use_slot) = inv_slot(bot, water)?;
        bot.op_held_u(clay, slot, com, water, use_slot, use_com)?;
        let target = bot.world.ticks + 40;
        while bot.world.ticks < target {
            if bot.world.modal.is_some() || !bot.chat_options().is_empty() || bot.world.count_dialog {
                return Err(ScriptError::Failed(format!("mixing {SOFT} opened an interface at {:?}", bot.position())));
            }
            bot.pump(2)?;
            if pack_total(bot, soft) > had {
                break;
            }
        }
        if pack_total(bot, soft) == had {
            return Err(ScriptError::Failed(format!("{SOFT} never mixed at {:?}", bot.position())));
        }
    }
    Ok(())
}

/// Top the pack up with clay and water, half and half (nothing when the
/// booth is far: the caller reports `blocked`).
fn fill_clay(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, clay: u16, water: u16) -> Result<(), ScriptError> {
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    if free_slots(bot) > 1 {
        bank_withdraw_items(bot, bank, &[clay], free_slots(bot) as u32 / 2)?;
        bank_withdraw_items(bot, bank, &[water], free_slots(bot) as u32)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extra_row, CLAY, OVENS, PRODUCT, SOFT, UNFIRED, WATER, WHEELS};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::grid_button;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6 pot chain from the dumps: mix (no XP), shape pot 63,
    /// fire pot 63; dishes and bowls sit higher.
    #[test]
    fn pot_chain_comes_from_gamedata() {
        let mix = extra_row("pottery_shape", SOFT).expect("mix row");
        assert_eq!((mix.level, mix.xp), (1, 0), "mix pays nothing");
        assert!(mix.inputs.iter().any(|(name, _)| name == CLAY));
        assert!(mix.inputs.iter().any(|(name, _)| name == WATER));
        let shape = extra_row("pottery_shape", UNFIRED).expect("shape row");
        assert_eq!((shape.level, shape.xp), (1, 63), "shape pot");
        assert_eq!(shape.station.as_deref(), Some("potterywheel"));
        let fire = extra_row("pottery_fire", PRODUCT).expect("fire row");
        assert_eq!((fire.level, fire.xp), (1, 63), "fire pot");
        assert_eq!(fire.station.as_deref(), Some("potteryoven"));
        assert!(extra_row("pottery_shape", "bowl_unfired").is_some(), "bowls wait for later");
    }

    /// Six wheels and two ovens stand free; the grid button and every
    /// pot name resolve through live tables.
    #[test]
    fn wheels_ovens_buttons_and_names_resolve() {
        assert_eq!(lifescape_gamedata::stations::stations_of(WHEELS[0]).len(), 6, "wheels");
        assert_eq!(lifescape_gamedata::stations::stations_of(OVENS[0]).len(), 2, "ovens");
        let bot = offline_bot();
        assert!(grid_button(&bot, "skill_multi3:makex_a").is_ok(), "pot make-x resolves");
        for name in [CLAY, WATER, SOFT, UNFIRED, PRODUCT] {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
    }
}
