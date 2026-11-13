//! Smelt: ores (+ coal) into bars at a furnace (catalogue §5.7, P1).
//! Ore on a furnace smelts one bar with no interface (the 274 `use`
//! path chains nothing), so the run goes through the shared produce
//! brick with a direct click per ore. The make-x grid is throughput
//! only, never correctness. Superheat stays out: `superheat` owns it.
//!
//! Process: poste fourneau · action fondre · interface directe ·
//! intrants minerais (+ charbon) · produits barres.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{self, Produce, ProduceJob, ProduceSite, ProduceUi};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const SMITHING_STAT: u8 = 13;
/// Furnace locs in preference order (big Smelt furnace first).
pub const FURNACES: [&str; 2] = ["furnace1", "furnace2"];
/// Highest F2P bar at each floor (catalogue §5.7): bar and smelt level.
pub const BARS: [(&str, i64); 8] = [("bronze_bar", 1), ("iron_bar", 15), ("silver_bar", 20), ("steel_bar", 30), ("gold_bar", 40), ("mithril_bar", 50), ("adamantite_bar", 70), ("runite_bar", 85)];

const SMELT: Produce<'static> =
    Produce { verb: "smelting", site: ProduceSite::Loc(&FURNACES), ui: ProduceUi::Direct };

/// F2P smelt recipe making `bar` (members excluded).
pub fn smelt_recipe(bar: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::making(bar, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// F2P smelt recipe eating `ore` (coal names steel, members excluded).
pub fn ore_recipe(ore: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(ore, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// Inputs of `recipe_inputs` the pack holds less of than one bar needs,
/// in recipe order. The server smelts only a full set (`smelting.rs2`
/// `validate_first_smelt`: primary ore AND secondary count, else a
/// failure box): copper alone makes nothing.
pub fn short_inputs(recipe_inputs: &[(String, i64)], carried: &dyn Fn(&str) -> i64) -> Vec<String> {
    recipe_inputs.iter().filter(|(name, each)| carried(name) < *each).map(|(name, _)| name.clone()).collect()
}

/// Furnace post for a run starting at (x, z, level): the nearest F2P
/// furnace on that level (Manhattan), Wilderness furnaces only with
/// `wild`. `None` when the level has none.
pub fn furnace_post(x: i64, z: i64, level: i64, allow_wild: bool) -> Option<knowledge::stations::Station> {
    knowledge::stations::stations_of("furnace")
        .into_iter()
        .filter(|post| post.level == level && (allow_wild || !knowledge::danger::in_wilderness(post.x, post.z, post.level)))
        .min_by_key(|post| ((post.x - x).abs() + (post.z - z).abs(), post.x, post.z))
}

/// Highest F2P bar smeltable at `level` (steel from 30, P1 rentable).
pub fn bar_for_level(level: i64) -> &'static str {
    BARS.iter().rev().find(|(_, need)| level >= *need).map(|(name, _)| *name).unwrap_or("bronze_bar")
}

/// Smelt banked loads in the open session: `smelt [bar|ore] [count]`
/// (best bar by level when omitted, 0 = forever; superheat excluded).
pub fn run_smelt(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("smelt", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, SMITHING_STAT, 1);
    let wanted = resource.as_deref().unwrap_or_else(|| bar_for_level(level));
    let recipe = smelt_recipe(wanted).or_else(|| ore_recipe(wanted)).ok_or_else(|| ScriptError::Usage(format!("no F2P smelt recipe eats or makes {wanted:?} at {:?}", bot.position())))?;
    let Some((bar_name, _)) = recipe.outputs.first() else {
        return Err(ScriptError::Usage(format!("smelt {wanted:?} makes nothing at {:?}", bot.position())));
    };
    if level < recipe.level {
        return Err(crate::kit::cycle::blocked_level("smithing", i64::from(recipe.level)));
    }
    // The ore used on the furnace: the server picks the bar from it
    // (`smelt_ore_single`: copper → bronze, iron + 2 coal → steel), never
    // from coal alone. Every other input rides in the same set.
    let Some((primary_name, _)) = recipe.inputs.iter().find(|(name, _)| *name != "coal") else {
        return Err(ScriptError::Usage(format!("smelt {bar_name} has no ore input at {:?}", bot.position())));
    };
    let mut inputs: Vec<SetInput> = Vec::new();
    for (name, each) in &recipe.inputs {
        inputs.push(SetInput { name: name.clone(), id: obj_id(bot, name)?, each: (*each).max(1) });
    }
    let primary = obj_id(bot, primary_name)?;
    let bar = obj_id(bot, bar_name)?;
    let mut bank = nearest_bank(bot)?;
    let (here_x, here_z, here_level) = (i64::from(bot.world.x), i64::from(bot.world.z), i64::from(bot.world.level));
    let post = furnace_post(here_x, here_z, here_level, spec.allow_wild)
        .ok_or_else(|| ScriptError::Usage(format!("no furnace on level {here_level} (wild={}) from {:?}", spec.allow_wild, bot.position())))?;
    bot.log(&format!("furnace post at ({},{})", post.x, post.z));
    // Ores carried in are banked with the rest and come back out as
    // balanced sets: a pack full of freshly mined copper would leave no
    // slot for the tin.
    restock(bot, &bank, &task_keep, &inputs, &recipe.inputs)?;
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if full_sets(step_bot, &inputs) == 0 {
            restock(step_bot, &bank, &task_keep, &inputs, &recipe.inputs)?;
        }
        // One use per full set: a stray ore past the last set would only
        // raise the server's "need tin/coal" box.
        let sets = full_sets(step_bot, &inputs) as u32;
        let job = ProduceJob { spec: &SMELT, input: primary, input_name: primary_name, want: sets, products: vec![bar], near: (post.x as i32, post.z as i32) };
        match produce::run_job(step_bot, &job) {
            Err(error) if is_death_abort(&error) => {
                step_bot.log("died smelting, restocking to retry");
                return Ok(StepControl::Continue);
            }
            Err(error) => return Err(error),
            Ok(batch) => {
                step_bot.log(&format!("ok smelted {} ({} made)", batch.consumed, batch.made));
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} smelted {bar_name} (smithing={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// One recipe input: name, object id, count per bar.
struct SetInput {
    name: String,
    id: u16,
    each: i64,
}

/// Full recipe sets the pack holds (the scarcest input decides).
fn full_sets(bot: &Bot, inputs: &[SetInput]) -> i64 {
    inputs.iter().map(|input| i64::from(pack_total(bot, input.id)) / input.each).min().unwrap_or(0)
}

/// Bank, top the pack up, and stop `blocked` on every input short of one
/// set (copper banked without tin names the tin; the brain files one
/// obtain per name).
fn restock(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, keep: &[u16], inputs: &[SetInput], recipe_inputs: &[(String, i64)]) -> Result<(), ScriptError> {
    bank_trip(bot, bank, keep, 0)?;
    fill_sets(bot, bank, inputs)?;
    let carried = |name: &str| inputs.iter().find(|input| input.name == name).map(|input| i64::from(pack_total(bot, input.id))).unwrap_or(0);
    let missing = short_inputs(recipe_inputs, &carried);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    Ok(())
}

/// Top the pack up to balanced sets: what the pack already carries
/// counts, each input is withdrawn up to `sets × each` (nothing when the
/// booth is far: the caller reports `blocked`).
fn fill_sets(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, inputs: &[SetInput]) -> Result<(), ScriptError> {
    if !crate::kit::withdraw::reach_bank_for(bot, bank, &inputs.iter().map(|input| input.id).collect::<Vec<u16>>())? {
        return Ok(());
    }
    let per_bar: i64 = inputs.iter().map(|input| input.each).sum();
    let carried: i64 = inputs.iter().map(|input| i64::from(pack_total(bot, input.id))).sum();
    let sets = (free_slots(bot) as i64 + carried) / per_bar.max(1);
    for input in inputs {
        let lacking = sets * input.each - i64::from(pack_total(bot, input.id));
        if lacking > 0 {
            bank_withdraw_items(bot, bank, &[input.id], lacking as u32)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bar_for_level, furnace_post, ore_recipe, short_inputs, smelt_recipe, BARS, FURNACES, SMITHING_STAT};

    /// Live 2026-11-13 (paul_rhodes, Falador; bh99tw2f, Al Kharid): 200
    /// copper banked, no tin; the run withdrew copper alone and used it on
    /// the furnace, the server answered its "need tin" box, and the job
    /// failed "consumed nothing" three times. A set is every input of the
    /// recipe; whatever falls short of one set is reported, so the brain
    /// files the tin (and the copper too when both are gone).
    #[test]
    fn a_smelt_set_needs_every_input() {
        let bronze = smelt_recipe("bronze_bar").expect("bronze").inputs;
        let carried = |pairs: &'static [(&'static str, i64)]| move |name: &str| pairs.iter().find(|(held, _)| *held == name).map(|(_, count)| *count).unwrap_or(0);
        assert_eq!(short_inputs(&bronze, &carried(&[("copper_ore", 14)])), vec!["tin_ore".to_string()], "copper alone smelts nothing");
        assert_eq!(short_inputs(&bronze, &carried(&[])), vec!["copper_ore".to_string(), "tin_ore".to_string()], "both ores asked");
        assert!(short_inputs(&bronze, &carried(&[("copper_ore", 1), ("tin_ore", 1)])).is_empty(), "one of each is a set");
        let steel = smelt_recipe("steel_bar").expect("steel").inputs;
        assert_eq!(short_inputs(&steel, &carried(&[("iron_ore", 9), ("coal", 1)])), vec!["coal".to_string()], "steel needs 2 coal per bar");
        assert!(short_inputs(&steel, &carried(&[("iron_ore", 1), ("coal", 2)])).is_empty());
        let iron = smelt_recipe("iron_bar").expect("iron").inputs;
        assert_eq!(short_inputs(&iron, &carried(&[])), vec!["iron_ore".to_string()]);
    }

    /// Live 2026-11-13 (bct5rxpg, eric_smith, b3gybcev at Varrock east,
    /// will_field at Edgeville): the post was picked near the bank but the
    /// job searched 150 tiles around the BANK, so every bot banking far
    /// from a furnace failed "no produce station". The post is the
    /// furnace nearest the bot, Wilderness only when allowed.
    #[test]
    fn furnace_post_is_the_nearest_furnace_outside_the_wilderness() {
        let near = |x: i64, z: i64| {
            let post = furnace_post(x, z, 0, false).expect("a furnace");
            (post.x, post.z)
        };
        let lumbridge = near(3253, 3420);
        assert!((lumbridge.0 - 3226).abs() <= 1 && (lumbridge.1 - 3255).abs() <= 1, "Varrock east smelts at Lumbridge, got {lumbridge:?}");
        let falador = near(2946, 3368);
        assert_eq!(falador, (2975, 3368), "Falador west bank smelts at Falador");
        let al_kharid = near(3269, 3167);
        assert!((al_kharid.0 - 3272).abs() <= 1 && (al_kharid.1 - 3185).abs() <= 1, "Al Kharid smelts at home, got {al_kharid:?}");
        assert_eq!(near(3096, 3493), (2975, 3368), "Edgeville goes to Falador, never the Wilderness furnace");
        let north = near(3140, 3600);
        assert!(north.1 < 3520, "deep north still avoids the Wilderness furnace, got {north:?}");
        let wild = furnace_post(3140, 3600, 0, true).expect("wild furnace");
        assert!(wild.z >= 3520, "wild allowed: the Wilderness furnace is nearest, got ({},{})", wild.x, wild.z);
    }
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.7 ladder comes from the dump (bronze 1 to runite 85,
    /// coal joining iron from steel on): levels, inputs and F2P flags.
    #[test]
    fn smelt_ladder_comes_from_gamedata() {
        assert_eq!(BARS.map(|(_, level)| level), [1, 15, 20, 30, 40, 50, 70, 85]);
        for (bar, level) in BARS {
            let recipe = smelt_recipe(bar).unwrap_or_else(|| panic!("{bar} recipe resolves"));
            assert_eq!(recipe.kind, "smelt");
            assert_eq!(recipe.level, level, "{bar} level");
            assert_eq!(recipe.members, Some(false), "{bar} F2P");
            assert_eq!(recipe.outputs, vec![(bar.to_string(), 1)]);
        }
        let bronze = smelt_recipe("bronze_bar").expect("bronze smelts");
        assert!(bronze.inputs.iter().any(|(name, _)| name == "copper_ore"), "copper in, got {:?}", bronze.inputs);
        assert!(bronze.inputs.iter().any(|(name, _)| name == "tin_ore"), "tin in, got {:?}", bronze.inputs);
        let steel = smelt_recipe("steel_bar").expect("steel smelts");
        assert!(steel.inputs.contains(&("iron_ore".to_string(), 1)) && steel.inputs.contains(&("coal".to_string(), 2)), "steel eats iron+2 coal, got {:?}", steel.inputs);
        let runite = smelt_recipe("runite_bar").expect("runite smelts");
        assert!(runite.inputs.contains(&("runite_ore".to_string(), 1)) && runite.inputs.contains(&("coal".to_string(), 8)), "runite eats ore+8 coal, got {:?}", runite.inputs);
        assert!(smelt_recipe("perfect_gold_bar").is_none(), "members perfect gold excluded");
        assert!(smelt_recipe("STEEL_BAR").is_some(), "case-insensitive");
        assert!(smelt_recipe("logs").is_none(), "logs never smelt");
    }

    /// Ore names resolve too (coal names steel); default pick is the
    /// highest bar at the level, never above.
    #[test]
    fn ore_lookup_and_default_pick() {
        let iron = ore_recipe("iron_ore").expect("iron smelts");
        assert!(iron.outputs.iter().any(|(name, _)| name == "iron_bar"), "iron gives iron_bar, got {:?}", iron.outputs);
        let steel = ore_recipe("coal").expect("coal smelts with iron");
        assert!(steel.outputs.iter().any(|(name, _)| name == "steel_bar"), "coal gives steel, got {:?}", steel.outputs);
        assert!(ore_recipe("logs").is_none(), "logs never smelt");
        assert_eq!(bar_for_level(1), "bronze_bar");
        assert_eq!(bar_for_level(19), "iron_bar");
        assert_eq!(bar_for_level(29), "silver_bar");
        assert_eq!(bar_for_level(30), "steel_bar");
        assert_eq!(bar_for_level(99), "runite_bar");
    }

    /// Furnace posts and inputs resolve: loc debugnames stand in the
    /// furnace dump (4 F2P sites incl. Al-Kharid and Wilderness), ores,
    /// coal and bars through live tables.
    #[test]
    fn furnace_posts_and_inputs_resolve() {
        let furnace_tiles = lifescape_gamedata::stations::stations_of("furnace");
        assert_eq!(furnace_tiles.len(), 8, "furnace tiles (4 sites)");
        for name in FURNACES {
            assert!(furnace_tiles.iter().any(|entry| entry.loc == *name), "furnace loc {name} placed");
        }
        assert!(furnace_tiles.iter().any(|entry| (entry.x - 3272).abs() <= 2 && (entry.z - 3185).abs() <= 2), "Al-Kharid furnace");
        assert!(furnace_tiles.iter().any(|entry| (entry.x - 3143).abs() <= 2 && (entry.z - 3735).abs() <= 2), "Wilderness furnace");
        let near = lifescape_gamedata::stations::nearest_station("furnace", 3270, 3180, 0).expect("Al-Kharid post");
        assert!((near.x - 3272).abs() <= 2 && (near.z - 3185).abs() <= 2);
        let bot = offline_bot();
        for (bar, _) in BARS {
            let recipe = smelt_recipe(bar).expect("recipe");
            for (name, _) in &recipe.inputs {
                assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
            }
            assert!(obj_id(&bot, bar).is_ok(), "{bar} resolves");
        }
        assert_eq!(SMITHING_STAT, 13, "provenance smithing id");
    }
}
