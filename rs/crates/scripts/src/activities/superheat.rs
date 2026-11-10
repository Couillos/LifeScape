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

use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
pub const SUPERHEAT_SPELL: &str = "superheat_item";
pub const SUPERHEAT_LEVEL: i64 = 43;

/// F2P smelt recipe eating `ore` (members excluded).
pub fn bar_for_ore(ore: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(ore, Some("smelt")).into_iter().find(|entry| entry.members == Some(false))
}

/// Runes of superheat from the dump.
pub fn superheat_runes() -> Vec<(String, i64)> {
    knowledge::spells::by_name(SUPERHEAT_SPELL).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Superheat banked ores in the open session: `superheat <ore> [count]`.
/// Resolves the bar, the level and the runes, then fails loud: no
/// spell-cast op in the SDK (dette P2.4).
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
    let bar = recipe.outputs.first().map(|(name, _)| name.clone()).unwrap_or_default();
    let _ = obj_id(bot, ore_name)?;
    let runes = superheat_runes();
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{SUPERHEAT_SPELL} has no runes in the dump at {:?}", bot.position())));
    }
    bot.log(&format!("{SUPERHEAT_SPELL} on {ore_name} for {bar}"));
    Err(ScriptError::Usage(format!("superheat needs a spell-cast op in the SDK (dette P2.4): cast {SUPERHEAT_SPELL} on {ore_name} with {runes:?} at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{bar_for_ore, superheat_runes, SUPERHEAT_LEVEL, SUPERHEAT_SPELL};

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
