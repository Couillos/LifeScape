//! Superheat: smelt ores to bars anywhere with 43 Magic (catalogue
//! §5.10, P2). The ore-to-bar declaration reuses the smelt recipes from
//! the dump; casting itself is a dette like alchemy: the SDK has no
//! spell-cast op outside combat, so the run resolves everything and
//! fails loud with the exact missing piece.
//!
//! Process: poste aucun (sort) · action superheat · interface sort ·
//! intrants minerai (+ charbon) + nature + feu · produits barres.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::magic::{cast_on_held, missing_runes};
use crate::kit::pack::{obj_id, pack_total};
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

/// Runes of superheat from the dump.
pub fn superheat_runes() -> Vec<(String, i64)> {
    knowledge::spells::by_name(SUPERHEAT_SPELL).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Superheat banked ores in the open session: `superheat <ore> [count]`
/// (count omitted or 0 = the pack). Missing runes or ore fail `Blocked`
/// (the brain files obtain tasks); each cast consumes the ore and pays
/// a bar plus the spell XP.
pub fn run_superheat(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("superheat", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let _ = crate::kit::cycle::resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    if level < SUPERHEAT_LEVEL {
        return Err(ScriptError::Usage(format!("{SUPERHEAT_SPELL} needs level {SUPERHEAT_LEVEL} at {:?}", bot.position())));
    }
    let ore_name = resource.as_deref().ok_or_else(|| ScriptError::Usage(format!("superheat <ore> [count], no ore at {:?}", bot.position())))?;
    let recipe = bar_for_ore(ore_name).ok_or_else(|| ScriptError::Usage(format!("no F2P smelt recipe eats {ore_name:?} at {:?}", bot.position())))?;
    let bar_name = recipe.outputs.first().map(|(name, _)| name.clone()).unwrap_or_default();
    let ore = obj_id(bot, ore_name)?;
    let bar = obj_id(bot, &bar_name)?;
    let runes = superheat_runes();
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{SUPERHEAT_SPELL} has no runes in the dump at {:?}", bot.position())));
    }
    let missing = missing_runes(bot, &runes);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    if pack_total(bot, ore) == 0 {
        return Err(ScriptError::Blocked { missing: vec![ore_name.to_string()] });
    }
    let want = spec.targets.count.filter(|count| *count > 0).unwrap_or(u32::MAX);
    let mut done = 0u32;
    while done < want {
        if pack_total(bot, ore) == 0 || !missing_runes(bot, &runes).is_empty() {
            break;
        }
        superheat_one(bot, ore, bar)?;
        done += 1;
    }
    bot.log(&format!("ok {SUPERHEAT_SPELL} x{done} on {ore_name} for {bar_name}"));
    Ok(0)
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
    }
}
