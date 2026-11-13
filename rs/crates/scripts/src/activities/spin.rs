//! Spinning: wool into a ball of wool on a spinning wheel (catalogue
//! §5.6, P1). PRD3 lot 7: the wheel's `op2` Spin opens the
//! `skill_multi2` make-menu (spinning.rs2), answered make-X with the whole
//! load, so the server chains one ball every 2 ticks instead of one click
//! per wool; the bank loop mirrors `cook`. Wool arrives from `shear`
//! (Sheep Shearer quest flock); flax is members and never spins here.
//!
//! Process: poste rouet (5) · action filer · interface directe · intrants
//! laine · produits pelote.

#![forbid(unsafe_code)]

use lifescape_gamedata::extra_recipes::ExtraRecipe;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const DEFAULT_INPUT: &str = "wool";
pub const PRODUCT: &str = "ball_of_wool";
/// Wheel debugname (catalogue §5.6: 5 posts, Lumbridge + Seers + Crafting Guild).
pub const STATIONS: [&str; 1] = ["spinningwheel"];

const SPIN: Produce<'static> = Produce {
    verb: "spinning",
    site: ProduceSite::Loc(&STATIONS),
    ui: ProduceUi::Multi { op: 2, menu: "skill_multi2", button: "skill_multi2:makex_a" },
};

/// F2P spin recipe eating `name` (wool only: flax stays members).
pub fn spin_recipe(name: &str) -> Option<ExtraRecipe> {
    let want = name.to_lowercase();
    lifescape_gamedata::extra_recipes::by_kind("spin").into_iter().find(|entry| entry.inputs.iter().any(|(input, _)| input.to_lowercase() == want))
}

/// Spin banked loads in the open session: `spin [input] [count]`
/// (wool when omitted, 0 = forever).
pub fn run_spin(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("spin", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let input_name = resource.as_deref().unwrap_or(DEFAULT_INPUT);
    let recipe = spin_recipe(input_name).ok_or_else(|| ScriptError::Usage(format!("no F2P spin recipe eats {input_name:?} at {:?}", bot.position())))?;
    if level < recipe.level {
        return Err(crate::kit::cycle::blocked_level("crafting", i64::from(recipe.level)));
    }
    let wool = obj_id(bot, input_name)?;
    let products: Vec<u16> = recipe.outputs.iter().filter_map(|(name, _)| obj_id(bot, name).ok()).collect();
    if products.is_empty() {
        return Err(ScriptError::Usage(format!("spin product of {input_name} unknown at {:?}", bot.position())));
    }
    // The wheel decides the site (PRD3 lot 7): the produce kit only
    // searches the bot's plane within 150 tiles of `near`, and the
    // nearest bank may have no wheel in range (Varrock east). Pick the
    // nearest same-plane F2P wheel, then the bank nearest to it.
    let (wheel, bank) = match wheel_site(bot) {
        Some(found) => found,
        None => (( bot.world.x, bot.world.z), nearest_bank(bot)?),
    };
    bot.log(&format!("wheel at ({},{}), bank at ({},{})", wheel.0, wheel.1, bank.x, bank.z));
    let mut keep_wool: Vec<u16> = vec![wool];
    keep_wool.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_wool, 0)?;
    fill_wool(bot, &bank, wool)?;
    if pack_total(bot, wool) == 0 {
        return Err(ScriptError::Blocked { missing: vec![input_name.to_string()] });
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, wool) == 0 {
            bank_trip(step_bot, &bank, &keep_wool, 0)?;
            fill_wool(step_bot, &bank, wool)?;
            if pack_total(step_bot, wool) == 0 {
                return Err(ScriptError::Blocked { missing: vec![input_name.to_string()] });
            }
        }
        let job = ProduceJob { spec: &SPIN, input: wool, input_name, want: 0, products: products.clone(), near: wheel };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died spinning, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => {
                step_bot.log(&format!("ok spun {} ({} made)", batch.consumed, batch.made));
            }
        }
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} spun (crafting={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

/// Nearest F2P wheel on the bot's plane and the bank nearest to it.
fn wheel_site(bot: &Bot) -> Option<((i32, i32), lifescape_gamedata::banks::Bank)> {
    let level = i64::from(bot.world.level);
    let wheel = lifescape_gamedata::stations::stations_of(STATIONS[0])
        .into_iter()
        .filter(|station| station.level == level && lifescape_gamedata::free_zones::is_free(station.x, station.z))
        .min_by_key(|station| (station.x - i64::from(bot.world.x)).abs() + (station.z - i64::from(bot.world.z)).abs())?;
    let bank = lifescape_gamedata::banks::nearest_bank(wheel.x, wheel.z, level)?;
    Some(((wheel.x as i32, wheel.z as i32), bank))
}

/// Top the pack up with wool from the bank (nothing when the bank holds
/// none or the booth is far: the caller reports `blocked`).
fn fill_wool(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, wool: u16) -> Result<(), ScriptError> {
    let free = free_slots(bot) as u32;
    if free == 0 {
        return Ok(());
    }
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &[wool])? {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[wool], free)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{spin_recipe, DEFAULT_INPUT, PRODUCT, STATIONS};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6: wool spins at level 1 for 25 XP into one ball;
    /// flax (members) has no F2P spin row.
    #[test]
    fn wool_spins_at_level_one() {
        let recipe = spin_recipe("wool").expect("wool spins");
        assert_eq!(recipe.kind, "spin");
        assert_eq!((recipe.level, recipe.xp), (1, 25));
        assert_eq!(recipe.inputs, vec![("wool".to_string(), 1)]);
        assert_eq!(recipe.outputs, vec![("ball_of_wool".to_string(), 1)]);
        assert_eq!(recipe.station.as_deref(), Some("spinningwheel"));
        assert!(spin_recipe("flax").is_none(), "flax stays members");
        assert!(spin_recipe("WOOL").is_some(), "case-insensitive");
    }

    /// Five wheels stand in F2P zones; wool and the ball resolve.
    #[test]
    fn wheels_and_wool_resolve() {
        assert_eq!(lifescape_gamedata::stations::stations_of(STATIONS[0]).len(), 5, "wheels");
        let bot = offline_bot();
        assert!(obj_id(&bot, DEFAULT_INPUT).is_ok(), "wool resolves");
        assert!(obj_id(&bot, PRODUCT).is_ok(), "ball of wool resolves");
    }
}
