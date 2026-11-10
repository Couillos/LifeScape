//! Alchemy: low (21) and high (55) alchemy on stackable loot
//! (catalogue §5.10). Declarations (levels, runes, alch values) come
//! from the dumps and are unit-tested; casting itself is a dette: the
//! SDK has no spell-cast op outside combat (`op_npc_t`), so the run
//! resolves everything and fails loud with the exact missing piece.
//!
//! Process: poste aucun (sort) · action alchimie · interface sort ·
//! intrants objet + nature + feu · produits pièces.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
pub const LOW_SPELL: &str = "lowlvl_alchemy";
pub const HIGH_SPELL: &str = "highlvl_alchemy";

/// Runes of a spell from the dump (case-insensitive debugname).
pub fn spell_runes(spell: &str) -> Vec<(String, i64)> {
    knowledge::spells::by_name(spell).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Alch values of `name` from the dump (`None` when it never alchs).
pub fn alch_value(name: &str) -> Option<(i64, i64)> {
    knowledge::objs_by_name(name).into_iter().find(|entry| entry.alch_high > 0).map(|entry| (entry.alch_high, entry.alch_low))
}

/// Alch a banked item in the open session: `alchemy <item> [count]`
/// (low below 55, high from 55). Resolves the spell, the level and the
/// runes, then fails loud: no spell-cast op in the SDK (dette P2.4).
pub fn run_alchemy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("alchemy", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let _ = crate::kit::cycle::resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    let item_name = resource.as_deref().ok_or_else(|| ScriptError::Usage(format!("alchemy <item> [count], no item at {:?}", bot.position())))?;
    let high = level >= 55;
    let spell = if high { HIGH_SPELL } else { LOW_SPELL };
    let need = if high { 55 } else { 21 };
    if level < need {
        return Err(ScriptError::Usage(format!("{spell} needs level {need} at {:?}", bot.position())));
    }
    let _ = obj_id(bot, item_name)?;
    let runes = spell_runes(spell);
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{spell} has no runes in the dump at {:?}", bot.position())));
    }
    let (worth_high, worth_low) = alch_value(item_name).ok_or_else(|| ScriptError::Usage(format!("{item_name} never alchs at {:?}", bot.position())))?;
    let worth = if high { worth_high } else { worth_low };
    bot.log(&format!("{spell} on {item_name} for {worth} coins"));
    Err(ScriptError::Usage(format!("alchemy needs a spell-cast op in the SDK (dette P2.4): cast {spell} on {item_name} with {runes:?} at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{alch_value, spell_runes, HIGH_SPELL, LOW_SPELL, MAGIC_STAT};

    /// Catalogue §5.10: low 21 (1 nature + 3 fire), high 55 (1 + 5).
    #[test]
    fn alchemy_levels_and_runes_come_from_gamedata() {
        let low = lifescape_gamedata::spells::by_name(LOW_SPELL);
        assert_eq!(low.len(), 1);
        assert_eq!((low[0].level, low[0].members), (21, false));
        let mut low_runes = spell_runes(LOW_SPELL);
        low_runes.sort();
        assert_eq!(low_runes, vec![("firerune".to_string(), 3), ("naturerune".to_string(), 1)]);
        let high = lifescape_gamedata::spells::by_name(HIGH_SPELL);
        assert_eq!(high.len(), 1);
        assert_eq!((high[0].level, high[0].members), (55, false));
        let mut high_runes = spell_runes(HIGH_SPELL);
        high_runes.sort();
        assert_eq!(high_runes, vec![("firerune".to_string(), 5), ("naturerune".to_string(), 1)]);
        assert!(spell_runes("no_such_spell").is_empty(), "unknown spell has no runes");
        assert_eq!(MAGIC_STAT, 6, "provenance magic id");
    }

    /// Alch values come from the object dump, junk never alchs.
    #[test]
    fn alch_values_come_from_the_dump() {
        assert_eq!(alch_value("yew_logs"), Some((96, 64)));
        assert!(alch_value("no_such_item_xyz").is_none(), "unknown never alchs");
    }
}
