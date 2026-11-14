//! Superheat: smelt ores to bars anywhere with 43 Magic (catalogue
//! §5.10, P2). The ore-to-bar declaration reuses the smelt recipes from
//! the dump; ores, coal and runes are stocked from the pack and the
//! known bank (`kit::spell_load`).
//!
//! Process: poste aucun (sort) · action superheat · interface sort ·
//! intrants minerai (+ charbon) + nature + feu · produits barres.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::cycle::exit_code;
use crate::kit::magic::{cast_on_held, wield_staff_for};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::spell_load::{run_casts, SpellLoad};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
pub const SUPERHEAT_SPELL: &str = "superheat_item";
pub const SUPERHEAT_LEVEL: i64 = 43;
/// Ticks to wait out one cast (server delays plus the smelt tick).
pub const CAST_TICKS: u64 = 40;

/// F2P smelt recipe eating `ore` (members excluded).
pub fn bar_for_ore(ore: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(ore, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// Ore the cast targets and the bar it lands, from the planner's bar
/// (`superheat:<bar>`: `iron_bar`, `steel_bar` → iron ore with the coal
/// carried) or from an ore name (manual runs). Coal is never the target.
pub fn ore_for(name: &str) -> Option<(String, String)> {
    let made = knowledge::recipes::making(name, Some("smelt")).into_iter().find(|entry| entry.members == Some(false));
    if let Some(recipe) = made {
        let ore = recipe.inputs.iter().map(|(input, _)| input.clone()).find(|input| input != "coal")?;
        return Some((ore, name.to_string()));
    }
    let recipe = bar_for_ore(name)?;
    let bar = recipe.outputs.first().map(|(output, _)| output.clone())?;
    Some((name.to_string(), bar))
}

/// Runes of superheat from the dump.
pub fn superheat_runes() -> Vec<(String, i64)> {
    knowledge::spells::by_name(SUPERHEAT_SPELL).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Superheat carried ores in the open session: `superheat <bar|ore>
/// [count] [--want ITEM N] [--level magic N]` (count omitted or 0 = all
/// held). Runes, ore or coal short in pack + bank fail `Blocked` by name
/// (the brain files obtain tasks); a worn, carried or banked fire staff
/// pays the fire runes. Each cast consumes the ore and pays a bar plus
/// the spell XP.
pub fn run_superheat(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("superheat", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let keep = crate::kit::cycle::resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    if level < SUPERHEAT_LEVEL {
        return Err(crate::kit::cycle::blocked_level("magic", i64::from(SUPERHEAT_LEVEL)));
    }
    let ordered = resource.as_deref().ok_or_else(|| ScriptError::Usage(format!("superheat <bar|ore> [count], nothing named at {:?}", bot.position())))?;
    let (ore_name, bar_name) = ore_for(ordered).ok_or_else(|| ScriptError::Usage(format!("no F2P smelt recipe makes or eats {ordered:?} at {:?}", bot.position())))?;
    let ore = obj_id(bot, &ore_name)?;
    let bar = obj_id(bot, &bar_name)?;
    let runes = superheat_runes();
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{SUPERHEAT_SPELL} has no runes in the dump at {:?}", bot.position())));
    }
    // Pack, then the known bank (`kit::spell_load`): the recipe's inputs
    // (steel: the ore plus two coal) ride per cast, a fire staff pays the
    // fire runes, the bars are banked between loads.
    let load = SpellLoad::new(SUPERHEAT_SPELL, runes, superheat_inputs(&ore_name, &bar_name));
    wield_staff_for(bot, &load.runes)?;
    let (done, stop) = run_casts(bot, &spec, &load, &keep, |cast_bot| superheat_one(cast_bot, ore, bar))?;
    bot.log(&format!("ok {SUPERHEAT_SPELL} x{done} on {ore_name} for {bar_name}"));
    stop.map_or(Ok(0), exit_code)
}

/// Per-cast inputs: the target ore first, then the smelt recipe's other
/// inputs (steel: 2 coal), from the dump.
pub fn superheat_inputs(ore: &str, bar: &str) -> Vec<(String, u32)> {
    let recipe = knowledge::recipes::making(bar, Some("smelt")).into_iter().find(|entry| entry.members == Some(false));
    let others = recipe.map(|entry| entry.inputs).unwrap_or_default().into_iter().filter(|(name, _)| name != ore);
    std::iter::once((ore.to_string(), 1)).chain(others.map(|(name, count)| (name, u32::try_from(count).unwrap_or(1)))).collect()
}

/// One cast: the ore leaves (or the bar lands) within the budget.
fn superheat_one(bot: &mut Bot, ore: u16, bar: u16) -> Result<(), ScriptError> {
    let had_ore = pack_total(bot, ore);
    let had_bar = pack_total(bot, bar);
    cast_on_held(bot, SUPERHEAT_SPELL, ore)?;
    let target = bot.world.ticks + CAST_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if pack_total(bot, ore) < had_ore || pack_total(bot, bar) > had_bar {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{SUPERHEAT_SPELL} never landed at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{bar_for_ore, run_superheat, superheat_runes, SUPERHEAT_LEVEL, SUPERHEAT_SPELL};
    use crate::kit::pack::{obj_id, pack_total};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Fund the pack through the live tables (pack com + slots).
    fn fund_pack(bot: &mut Bot, stock: &[(u8, &str, u32)]) {
        let com = (bot.tables.com_id)("inventory:inv").expect("pack com in dumps");
        let slots: std::collections::HashMap<u8, (u16, u32)> = stock
            .iter()
            .map(|(slot, name, count)| (*slot, (obj_id(bot, name).expect("funded obj resolves"), *count)))
            .collect();
        bot.world.inv.insert(com, slots);
        bot.world.inv_cap.insert(com, 28);
    }

    /// Runes starve loud: an unfunded pack blocks with the rune names.
    #[test]
    fn unfunded_pack_blocks_on_runes() {
        let mut bot = offline_bot();
        bot.world.stats.insert(super::MAGIC_STAT, (43, 0));
        fund_pack(&mut bot, &[(0, "iron_ore", 1)]);
        let error = run_superheat(&mut bot, &["iron_ore".to_string()]).expect_err("no runes, no cast");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got {error}");
        assert!(format!("{error}").contains("naturerune"), "names the rune, got {error}");
    }

    /// Ore and runes banked, nothing carried: the run goes to the bank
    /// (offline: the walk fails typed), never `blocked` on what the bank
    /// holds. Steel needs its two coal per cast, named when short.
    #[test]
    fn banked_load_sends_the_run_to_the_bank() {
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[], &[("naturerune", 5), ("firerune", 20), ("iron_ore", 5)]);
        bot.world.stats.insert(super::MAGIC_STAT, (43, 0));
        bot.world.x = 3222;
        bot.world.z = 3218;
        let error = run_superheat(&mut bot, &["iron_bar".to_string()]).expect_err("offline bank walk fails");
        assert!(!matches!(error, crate::ScriptError::Blocked { .. }), "the bank holds the load, got {error}");
        let mut bot = crate::kit::spell_load::tests::stocked_bot(&[("coal", 1)], &[("naturerune", 5), ("firerune", 20), ("iron_ore", 5)]);
        bot.world.stats.insert(super::MAGIC_STAT, (43, 0));
        match run_superheat(&mut bot, &["steel_bar".to_string()]) {
            Err(crate::ScriptError::Blocked { missing }) => assert_eq!(missing, vec!["coal".to_string()]),
            other => panic!("blocked on coal only, got {other:?}"),
        }
    }

    /// A funded cast reaches the send brick: dead socket fails `Sdk`,
    /// never the old dette `Usage`.
    #[test]
    fn funded_cast_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(super::MAGIC_STAT, (43, 0));
        fund_pack(&mut bot, &[(0, "iron_ore", 1), (1, "naturerune", 1), (2, "firerune", 4)]);
        let ore = obj_id(&bot, "iron_ore").expect("ore resolves");
        let error = run_superheat(&mut bot, &["iron_ore".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        assert_eq!(pack_total(&bot, ore), 1, "nothing consumed offline");
    }

    /// Catalogue §5.10: superheat at 43 (1 nature + 4 fire), F2P.
    #[test]
    fn superheat_level_and_runes_come_from_gamedata() {
        let spells = lifescape_gamedata::spells::by_name(SUPERHEAT_SPELL);
        assert_eq!(spells.len(), 1);
        assert_eq!((spells[0].level, spells[0].members), (SUPERHEAT_LEVEL, false));
        let mut runes = superheat_runes();
        runes.sort();
        assert_eq!(runes, vec![("firerune".to_string(), 4), ("naturerune".to_string(), 1)]);
    }

    /// Ore-to-bar declarations reuse the smelt dump: iron smelts alone,
    /// coal joins iron for steel, gold and mithril resolve F2P.
    #[test]
    fn ore_to_bar_comes_from_smelt_recipes() {
        let iron = bar_for_ore("iron_ore").expect("iron smelts");
        assert!(iron.outputs.iter().any(|(name, _)| name == "iron_bar"), "iron gives iron_bar, got {:?}", iron.outputs);
        let steel = bar_for_ore("coal").expect("coal smelts with iron");
        assert!(steel.outputs.iter().any(|(name, _)| name == "steel_bar"), "coal gives steel, got {:?}", steel.outputs);
        assert!(bar_for_ore("gold_ore").is_some(), "gold resolves F2P");
        assert!(bar_for_ore("GOLD_ORE").is_some(), "case-insensitive");
        assert!(bar_for_ore("logs").is_none(), "logs never smelt");
        // The planner orders the BAR (`superheat:<bar>`): the cast eats its
        // ore, coal rides along, never the target.
        let pair = |ore: &str, bar: &str| Some((ore.to_string(), bar.to_string()));
        assert_eq!(super::ore_for("iron_bar"), pair("iron_ore", "iron_bar"));
        assert_eq!(super::ore_for("steel_bar"), pair("iron_ore", "steel_bar"));
        assert_eq!(super::ore_for("iron_ore"), pair("iron_ore", "iron_bar"), "manual ore runs keep working");
        assert_eq!(super::ore_for("logs"), None);
    }
}
