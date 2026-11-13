//! Dyes: Aggie's paid service in Draynor (catalogue §5.6, P1). Onion
//! skins, redberries or woad leaves used on Aggie turn into dye for 5
//! coins a bottle (direct click, no modal: any interface is loud, like
//! stringing). Woad leaves come from Wyson in Falador (15 coins a leaf
//! through his haggle dialogue), which also feeds Goblin Diplomacy; the
//! Crafting Guild (40 + brown apron) hosts a wheel and a bank nearby.
//!
//! Process: poste PNJ aggie/wyson · action teindre/acheter · interface
//! directe + choix · intrants oignons/baies/guède + pièces · produits
//! teintures.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const AGGIE: &str = "aggie";
pub const WYSON: &str = "wyson";
pub const COINS: &str = "coins";
pub const FEE_GP: u32 = 5;
pub const WOAD_PRICE_GP: u32 = 15;
pub const DEFAULT_DYE: &str = "yellowdye";
/// Dye rows: ingredient, needed, product (Aggie's verified recipe).
pub const DYES: [(&str, u32, &str); 3] = [("onion", 2, "yellowdye"), ("redberries", 3, "reddye"), ("woadleaf", 2, "bluedye")];

const DYEING: Produce<'static> = Produce { verb: "dyeing", site: ProduceSite::Npc(&[AGGIE]), ui: ProduceUi::Direct };

/// Dye row by product or ingredient name (`None` for anything else).
pub fn dye_row(name: &str) -> Option<(&'static str, u32, &'static str)> {
    DYES.iter().find(|(ingredient, _, product)| ingredient.eq_ignore_ascii_case(name) || product.eq_ignore_ascii_case(name)).copied()
}

/// Make banked loads of dye in the open session: `dye [dye] [count]`
/// (yellow when omitted, 0 = forever). A missing ingredient blocks by
/// name (woad leaves: Wyson, through the brain's obtain leg).
pub fn run_dye(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("dye", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let dye_name = resource.as_deref().unwrap_or(DEFAULT_DYE);
    let (ingredient_name, _, product_name) = dye_row(dye_name).ok_or_else(|| ScriptError::Usage(format!("no F2P dye named {dye_name:?} at {:?}", bot.position())))?;
    let ingredient = obj_id(bot, ingredient_name)?;
    let coins = obj_id(bot, COINS)?;
    let _ = obj_id(bot, product_name)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_dye: Vec<u16> = vec![ingredient, coins];
    keep_dye.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_dye, 0)?;
    fill_dye(bot, &bank, ingredient, coins)?;
    // Woad leaves come from Wyson through the brain's obtain leg, never
    // a purchase inside this script: a missing ingredient blocks by name.
    if pack_total(bot, ingredient) == 0 {
        return Err(ScriptError::Blocked { missing: vec![ingredient_name.to_string()] });
    }
    run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, ingredient) == 0 {
            bank_trip(step_bot, &bank, &keep_dye, 0)?;
            fill_dye(step_bot, &bank, ingredient, coins)?;
            if pack_total(step_bot, ingredient) == 0 {
                return Err(ScriptError::Blocked { missing: vec![ingredient_name.to_string()] });
            }
        }
        if pack_total(step_bot, coins) < FEE_GP {
            return Err(ScriptError::Blocked { missing: vec![COINS.to_string()] });
        }
        let job = ProduceJob { spec: &DYEING, input: ingredient, input_name: ingredient_name, want: 0, products: vec![obj_id(step_bot, product_name)?], near: (bank.x as i32, bank.z as i32) };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died dyeing, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => {
                step_bot.log(&format!("ok dyed {} ({} made)", batch.consumed, batch.made));
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} dyed {product_name}", state.units() + 1));
        Ok(StepControl::UnitDone)
    })
}

/// Top the pack up with the ingredient, topping coins up to 100 first
/// (nothing when the booth is far: the caller reports `blocked`).
fn fill_dye(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, ingredient: u16, coins: u16) -> Result<(), ScriptError> {
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &[ingredient, coins])? {
        return Ok(());
    }
    if pack_total(bot, coins) < 25 {
        bank_withdraw_items(bot, bank, &[coins], 100)?;
    }
    bank_withdraw_items(bot, bank, &[ingredient], free_slots(bot) as u32)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{dye_row, AGGIE, COINS, DEFAULT_DYE, DYES, FEE_GP, WOAD_PRICE_GP, WYSON};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_gamedata::services::{all_services, GUILDS};
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6 recipes: 2 onions, 3 berries, 2 woad leaves, 5 coins
    /// each; both spellings and ingredients resolve to their row.
    #[test]
    fn dye_rows_match_aggies_recipe() {
        assert_eq!(DYES.len(), 3);
        assert_eq!(dye_row("yellowdye"), Some(("onion", 2, "yellowdye")));
        assert_eq!(dye_row("redberries"), Some(("redberries", 3, "reddye")));
        assert_eq!(dye_row("woadleaf"), Some(("woadleaf", 2, "bluedye")));
        assert_eq!(dye_row("ONION"), dye_row("yellowdye"), "ingredient alias");
        assert!(dye_row("greendye").is_none(), "members mix never dyes");
        assert!(dye_row(DEFAULT_DYE).is_some(), "default dyes");
        assert_eq!((FEE_GP, WOAD_PRICE_GP), (5, 15), "verified service prices");
    }

    /// Aggie and Wyson stand in F2P zones with the verified price tags;
    /// the Crafting Guild gates at 40 with a brown apron.
    #[test]
    fn services_and_guild_match_the_dumps() {
        let services = all_services();
        for (key, cost) in [("aggie_red", 5), ("aggie_yellow", 5), ("aggie_blue", 5), ("wyson_woad", 15)] {
            let found = services.iter().find(|entry| entry.key == key).unwrap_or_else(|| panic!("{key} placed"));
            assert_eq!(found.cost_gp, cost, "{key} price");
        }
        assert!(GUILDS.contains(&("crafting_guild", 40, "brown_apron")), "guild 40");
        assert!(!lifescape_gamedata::npcs_by_name(AGGIE).is_empty(), "aggie placed");
        assert!(!lifescape_gamedata::npcs_by_name(WYSON).is_empty(), "wyson placed");
    }

    /// Every ingredient, dye and coin name resolves through live tables.
    #[test]
    fn dye_names_resolve() {
        let bot = offline_bot();
        assert!(obj_id(&bot, COINS).is_ok(), "coins resolve");
        for (ingredient, _, product) in DYES {
            assert!(obj_id(&bot, ingredient).is_ok(), "{ingredient} resolves");
            assert!(obj_id(&bot, product).is_ok(), "{product} resolves");
        }
    }
}
