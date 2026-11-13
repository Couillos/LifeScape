//! Composed meals: pies, stew, pizza and cake on a range, wine by
//! fermentation (catalogue §5.4, all P2). The uncooked stages assemble
//! in the pack from banked components; the bake runs through `produce`
//! like `cook`. The cake tin comes from ground spawns only (Cooks'
//! guild, Wilderness): never bought, a missing tin blocks by name.
//! Serveur : pies.rs2, stew.rs2, pizza.rs2, cakes.rs2, wine.rs2.
//!
//! Process: poste cuisinière (32, feux 20) · action cuire · interface
//! directe · intrants plats crus · produits plats.
//! shortcut: meat/anchovy/chocolate toppings stay unbaked (plain bases
//! banked); add a topping activity when the brain orders finished pizzas.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::harvest::dismiss_modal;
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::{ScriptError, ScriptResult};

pub const COOK_STAT: u8 = 7;
pub const TIN: &str = "cake_tin";

/// A priori yield of one meal load cycle, replaced by measured rates.
pub const PRIOR_MEALS_PER_6000: u64 = 100;
pub const PRIOR_XP_PER_6000: u64 = 78_000;

/// One composed meal: the baked input and its bake gate. Wine ferments
/// in the pack and never touches a range.
pub struct Meal {
    pub key: &'static str,
    pub input: &'static str,
    pub level: i64,
    pub wine: bool,
}

pub const MEALS: [Meal; 7] = [
    Meal { key: "redberry_pie", input: "uncooked_redberry_pie", level: 10, wine: false },
    Meal { key: "meat_pie", input: "uncooked_meat_pie", level: 20, wine: false },
    Meal { key: "stew", input: "uncooked_stew", level: 25, wine: false },
    Meal { key: "apple_pie", input: "uncooked_apple_pie", level: 30, wine: false },
    Meal { key: "pizza", input: "uncooked_pizza", level: 35, wine: false },
    Meal { key: "wine", input: "jug_unfermented_wine", level: 35, wine: true },
    Meal { key: "cake", input: "uncooked_cake", level: 40, wine: false },
];

const BAKE: Produce<'static> =
    Produce { verb: "baking", site: ProduceSite::Loc(&super::cook::STATIONS), ui: ProduceUi::Direct };
const ITEM_TICKS: u64 = 40;
const FERMENT_TICKS: u64 = 60;

/// Meal by key, uncooked input or finished product (case-insensitive).
pub fn meal_for(name: &str) -> Option<&'static Meal> {
    let lower = name.to_lowercase();
    MEALS.iter().find(|meal| meal.key == lower || meal.input == lower || (meal.key == "pizza" && lower == "plain_pizza") || (meal.key == "wine" && lower == "jug_wine"))
}

/// Bake banked loads in the open session: `cook_meals [meal] [count]`
/// (redberry pie when omitted, 0 = forever).
pub fn run_cook_meals(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let meal_name = args.first().filter(|first| first.parse::<u32>().is_err() && !first.starts_with("--") && *first != "toll" && *first != "wild").cloned();
    let rest: Vec<String> = if meal_name.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("cook_meals", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let meal = meal_for(meal_name.as_deref().unwrap_or("redberry_pie"))
        .ok_or_else(|| ScriptError::Usage(format!("no F2P composed meal named {meal_name:?} at {:?}", bot.position())))?;
    let level = learn_level(bot, COOK_STAT, 1);
    if level < meal.level {
        return Err(crate::kit::cycle::blocked_level("cooking", i64::from(meal.level)));
    }
    let mut products: Vec<u16> = Vec::new();
    if !meal.wine {
        let recipe = produce::cook_recipe(meal.input)
            .ok_or_else(|| ScriptError::Usage(format!("no F2P cook recipe bakes {:?} at {:?}", meal.input, bot.position())))?;
        products = recipe.outputs.iter().filter_map(|(name, _)| obj_id(bot, name).ok()).collect();
        if let Some(burnt) = recipe.burnt.as_deref() {
            if let Ok(id) = obj_id(bot, burnt) { products.push(id); }
        }
    }
    let input = obj_id(bot, meal.input)?;
    let parts: Vec<u16> = parts_of(meal).iter().filter_map(|name| obj_id(bot, name).ok()).collect();
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_all: Vec<u16> = vec![input];
    keep_all.extend(parts.iter().copied());
    for half in ["pastry_dough", "pie_shell", "pizza_base", "incomplete_pizza", "stew1", "stew2"] {
        if let Ok(id) = obj_id(bot, half) { keep_all.push(id); }
    }
    keep_all.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_all, 0)?;
    fill_meal(bot, &bank, input, &parts)?;
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, input) == 0 {
            bank_trip(step_bot, &bank, &keep_all, 0)?;
            fill_meal(step_bot, &bank, input, &parts)?;
            if pack_total(step_bot, input) == 0 {
                prep_meal(step_bot, &bank, &keep_all, meal)?;
            }
            if pack_total(step_bot, input) == 0 {
                let mut missing = vec![meal.input.to_string()];
                for name in parts_of(meal) {
                    if obj_id(step_bot, name).is_ok_and(|id| pack_total(step_bot, id) == 0) {
                        missing.push((*name).to_string());
                    }
                }
                return Err(ScriptError::Blocked { missing });
            }
        }
        if meal.wine {
            let had = pack_total(step_bot, input);
            let until = step_bot.world.ticks + FERMENT_TICKS;
            while step_bot.world.ticks < until && pack_total(step_bot, input) > 0 {
                check_events(step_bot)?;
                step_bot.pump(2)?;
            }
            if pack_total(step_bot, input) >= had {
                return Err(ScriptError::Failed(format!("wine never fermented at {:?}", step_bot.position())));
            }
        } else {
            let job = ProduceJob { spec: &BAKE, input, input_name: meal.input, want: 0, products: products.clone(), near: (bank.x as i32, bank.z as i32) };
            match produce::run_job(step_bot, &job) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died baking, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(batch) => step_bot.log(&format!("ok baked {} ({} made)", batch.consumed, batch.made)),
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} {}d (cooking={level})", state.units() + 1, meal.key));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Banked components per meal, in prep order (flour and water first:
/// they mix into the dough the rest builds on).
fn parts_of(meal: &Meal) -> &'static [&'static str] {
    match meal.key {
        "redberry_pie" => &["pot_flour", "jug_water", "piedish", "redberries"], "meat_pie" => &["pot_flour", "jug_water", "piedish", "cooked_meat"], "stew" => &["bowl_water", "potato", "cooked_meat"],
        "apple_pie" => &["pot_flour", "jug_water", "piedish", "cooking_apple"], "pizza" => &["pot_flour", "jug_water", "tomato", "cheese"], "wine" => &["grapes", "jug_water"],
        _ => &["pot_flour", "egg", "bucket_milk", TIN],
    }
}

/// Top the pack with the uncooked stage first, then split the rest
/// across components (one slot stays free for prep clicks).
fn fill_meal(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, input: u16, parts: &[u16]) -> Result<(), ScriptError> {
    bank_withdraw_items(bot, bank, &[input], free_slots(bot).saturating_sub(1) as u32)?;
    for (slot, part) in parts.iter().enumerate() {
        bank_withdraw_items(bot, bank, &[*part], free_slots(bot).saturating_sub(1) as u32 / (parts.len() - slot) as u32)?;
    }
    Ok(())
}

/// Assemble the uncooked stage in the pack, one pour per server click
/// order (pies share pastry, dish, filling; stew pours skip when a side
/// is absent, so meat-first and potato-first both complete).
fn prep_meal(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], meal: &Meal) -> Result<(), ScriptError> {
    match meal.key {
        "redberry_pie" => { pour(bot, bank, keep, "pot_flour", "jug_water", Some("Pastry dough"))?; pour(bot, bank, keep, "pastry_dough", "piedish", None)?; pour(bot, bank, keep, "redberries", "pie_shell", None)?; }
        "meat_pie" => { pour(bot, bank, keep, "pot_flour", "jug_water", Some("Pastry dough"))?; pour(bot, bank, keep, "pastry_dough", "piedish", None)?; pour(bot, bank, keep, "cooked_meat", "pie_shell", None)?; }
        "apple_pie" => { pour(bot, bank, keep, "pot_flour", "jug_water", Some("Pastry dough"))?; pour(bot, bank, keep, "pastry_dough", "piedish", None)?; pour(bot, bank, keep, "cooking_apple", "pie_shell", None)?; }
        "stew" => {
            pour(bot, bank, keep, "cooked_meat", "bowl_water", None)?;
            pour(bot, bank, keep, "potato", "stew2", None)?;
            pour(bot, bank, keep, "potato", "bowl_water", None)?;
            pour(bot, bank, keep, "cooked_meat", "stew1", None)?;
        }
        "pizza" => {
            pour(bot, bank, keep, "pot_flour", "jug_water", Some("Pizza dough"))?;
            pour(bot, bank, keep, "pizza_base", "tomato", None)?;
            pour(bot, bank, keep, "cheese", "incomplete_pizza", None)?;
        }
        "wine" => pour(bot, bank, keep, "grapes", "jug_water", None)?,
        _ => pour(bot, bank, keep, "egg", TIN, None)?,
    }
    Ok(())
}

/// Click every `held` on `target` while both ride along (dough choice
/// answered). A full pack banks the empties first: dough nets one item
/// a mix, the rest shrink the pack.
pub(crate) fn pour(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], held_name: &str, target_name: &str, pick: Option<&str>) -> Result<(), ScriptError> {
    let (held, target) = (obj_id(bot, held_name)?, obj_id(bot, target_name)?);
    while pack_total(bot, held) > 0 && pack_total(bot, target) > 0 {
        check_events(bot)?;
        if free_slots(bot) == 0 {
            bank_trip(bot, bank, keep, 0)?;
        }
        click_inv(bot, held, target, pick)?;
    }
    Ok(())
}

/// Click `held` on `target` in the pack. Level-up modals clear; any
/// other interface is loud. Done when the held stack drops: every
/// assembly here consumes it.
fn click_inv(bot: &mut Bot, held: u16, target: u16, pick: Option<&str>) -> Result<(), ScriptError> {
    let had = pack_total(bot, held);
    let (com, slot) = inv_slot(bot, held)?;
    let (use_com, use_slot) = inv_slot(bot, target)?;
    bot.op_held_u(held, slot, com, target, use_slot, use_com)?;
    let until = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < until {
        if !bot.chat_options().is_empty() {
            let Some(choice) = pick else {
                return Err(ScriptError::Failed(format!("unexpected choice at {:?}", bot.position())));
            };
            if produce::match_option(&bot.chat_options(), choice).is_none() {
                return Err(ScriptError::Failed(format!("choice {choice:?} matches no option at {:?}", bot.position())));
            }
            bot.chat_choose(choice)?;
            bot.pump(5)?;
        }
        if bot.world.modal.is_some() && !dismiss_modal(bot)? {
            break;
        }
        bot.pump(2)?;
        if pack_total(bot, held) < had {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("nothing mixed at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{meal_for, MEALS, TIN};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::cook_recipe;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.4: every bake input has its F2P cook row at the
    /// catalogue level and XP, with a burnt byproduct.
    #[test]
    fn meals_bake_from_gamedata() {
        for (input, level, xp, product) in [
            ("uncooked_redberry_pie", 10, 780, "redberry_pie"),
            ("uncooked_meat_pie", 20, 1100, "meat_pie"),
            ("uncooked_stew", 25, 1170, "stew"),
            ("uncooked_apple_pie", 30, 1300, "apple_pie"),
            ("uncooked_pizza", 35, 1430, "plain_pizza"),
            ("uncooked_cake", 40, 1800, "cake"),
        ] {
            let recipe = cook_recipe(input).unwrap_or_else(|| panic!("{input} row resolves"));
            assert_eq!(recipe.kind, "cook", "{input} kind");
            assert_eq!((recipe.level, recipe.xp), (level, xp), "{input} gate and pay");
            assert!(recipe.outputs.iter().any(|(name, _)| name == product), "{input} makes {product}");
            assert!(recipe.burnt.is_some(), "{input} burns");
            assert_eq!(recipe.members, Some(false), "{input} F2P");
            let meal = MEALS.iter().find(|meal| meal.input == input).unwrap_or_else(|| panic!("{input} in MEALS"));
            assert_eq!((meal.level, meal.wine), (level, false), "{input} gate matches the dump");
        }
    }

    /// Meal keys resolve by key, uncooked input, product and case.
    #[test]
    fn meal_keys_resolve() {
        assert_eq!(meal_for("stew").expect("stew").input, "uncooked_stew");
        assert_eq!(meal_for("UNCOOKED_STEW").expect("input").key, "stew", "case-insensitive");
        assert_eq!(meal_for("plain_pizza").expect("plain").key, "pizza", "product alias");
        assert_eq!(meal_for("jug_wine").expect("jug").key, "wine", "product alias");
        assert_eq!(MEALS.len(), 7);
        assert!(meal_for("curry").is_none(), "members curry never resolves");
        assert!(meal_for("coins").is_none(), "junk never resolves");
    }

    /// Wine ferments in the pack (cook_prep, L35 1100 XP at ferment);
    /// grapes, jugs and both wines resolve.
    #[test]
    fn wine_ferments_from_gamedata() {
        let wine = lifescape_gamedata::extra_recipes::by_kind("cook_prep")
            .into_iter()
            .find(|entry| entry.source == "ferment_wine")
            .expect("ferment row");
        assert_eq!((wine.level, wine.xp), (35, 1100));
        assert!(wine.inputs.iter().any(|(name, _)| name == "grapes"));
        assert!(wine.inputs.iter().any(|(name, _)| name == "jug_water"));
        let bot = offline_bot();
        for name in ["grapes", "jug_water", "jug_unfermented_wine", "jug_wine"] {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
    }

    /// Every component and half-built stage resolves F2P; the tin has
    /// its 2 guild spawns and no F2P shop stocks it (spawns only).
    #[test]
    fn components_and_tin_resolve() {
        let bot = offline_bot();
        let halves = ["pastry_dough", "pie_shell", "pizza_base", "incomplete_pizza", "stew1", "stew2"];
        for meal in MEALS.iter() {
            for name in super::parts_of(meal).iter().copied().chain(halves) {
                let found = lifescape_gamedata::objs_by_name(name).into_iter().next().unwrap_or_else(|| panic!("{name} in dumps"));
                assert_eq!(found.members, Some(false), "{name} F2P");
                assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
            }
        }
        let tin = lifescape_gamedata::objs_by_name(TIN).into_iter().next().expect("tin in dumps");
        assert_eq!(tin.members, Some(false), "tin F2P");
        assert_eq!(lifescape_gamedata::f2p::f2p_obj_spawns(Some(tin.obj_id)).len(), 2, "tin spawns only");
        assert!(
            lifescape_gamedata::f2p::f2p_shops().iter().flat_map(|shop| shop.stocks.iter()).all(|stock| stock.obj != TIN),
            "no F2P shop sells the tin"
        );
    }
}
