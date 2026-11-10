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
        return Err(ScriptError::Usage(format!("smelting {bar_name} needs level {} at {:?}", recipe.level, bot.position())));
    }
    let Some((primary_name, primary_each)) = recipe.inputs.iter().find(|(name, _)| *name != "coal") else {
        return Err(ScriptError::Usage(format!("smelt {bar_name} has no ore input at {:?}", bot.position())));
    };
    let secondary: Vec<(String, i64)> =
        recipe.inputs.iter().filter(|(name, _)| name != primary_name).map(|(name, count)| (name.clone(), *count)).collect();
    let per_bar: i64 = primary_each + secondary.iter().map(|(_, count)| *count).sum::<i64>();
    let primary = obj_id(bot, primary_name)?;
    let mut secondary_ids: Vec<(u16, u32)> = Vec::new();
    for (name, count) in &secondary {
        secondary_ids.push((obj_id(bot, name)?, *count as u32));
    }
    let bar = obj_id(bot, bar_name)?;
    let mut bank = nearest_bank(bot)?;
    let post = knowledge::stations::nearest_station("furnace", bank.x, bank.z, bank.level).ok_or_else(|| ScriptError::Usage(format!("no furnace post near bank at {:?}", bot.position())))?;
    bot.log(&format!("furnace post at ({},{})", post.x, post.z));
    let mut keep_smelt: Vec<u16> = vec![primary];
    keep_smelt.extend(secondary_ids.iter().map(|(id, _)| *id));
    keep_smelt.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_smelt, 0)?;
    fill_ores(bot, &bank, primary, &secondary_ids, *primary_each, per_bar)?;
    if pack_total(bot, primary) == 0 {
        return Err(ScriptError::Blocked { missing: vec![primary_name.clone()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, primary) == 0 {
            bank_trip(step_bot, &bank, &keep_smelt, 0)?;
            fill_ores(step_bot, &bank, primary, &secondary_ids, *primary_each, per_bar)?;
            if pack_total(step_bot, primary) == 0 {
                return Err(ScriptError::Blocked { missing: vec![primary_name.clone()] });
            }
        }
        let job = ProduceJob { spec: &SMELT, input: primary, input_name: primary_name, want: 0, products: vec![bar], near: (bank.x as i32, bank.z as i32) };
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

/// Top the pack up with balanced ore sets (nothing when the bank holds
/// none or the booth is far: the caller reports `blocked`).
fn fill_ores(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, primary: u16, secondary: &[(u16, u32)], primary_each: i64, per_bar: i64) -> Result<(), ScriptError> {
    let free = free_slots(bot) as i64;
    if free == 0 || (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    let sets = free / per_bar;
    if sets == 0 {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[primary], (sets * primary_each) as u32)?;
    for (id, each) in secondary {
        bank_withdraw_items(bot, bank, &[*id], (sets * *each as i64) as u32)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bar_for_level, ore_recipe, smelt_recipe, BARS, FURNACES, SMITHING_STAT};
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
