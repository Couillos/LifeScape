//! Enchant and travel magic: amulet enchantment (7/27/49/57), the three
//! F2P teleports (25/31/37) and bones to bananas (15), all catalogue
//! §5.10. Levels and runes come from the spell dump and are
//! unit-tested; casting is a dette like alchemy (no spell-cast op in
//! the SDK), so the run resolves spell, level, runes and target, then
//! fails loud with the exact missing piece.
//!
//! Process: poste aucun (sort) · action enchanter/téléporter ·
//! interface sort · intrants amulette/os + runes · produits amulettes
//! enchantées / destination / bananes.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
/// F2P enchant ladder: spell, level, strung input, enchanted product.
pub const ENCHANTS: [(&str, i64, &str, &str); 4] = [
    ("enchant_lvl1", 7, "strung_sapphire_amulet", "amulet_of_magic"),
    ("enchant_lvl2", 27, "strung_emerald_amulet", "amulet_of_defence"),
    ("enchant_lvl3", 49, "strung_ruby_amulet", "amulet_of_strength"),
    ("enchant_lvl4", 57, "strung_diamond_amulet", "amulet_of_power"),
];
/// F2P teleports: spell and level (catalogue §5.10).
pub const TELEPORTS: [(&str, i64); 3] =
    [("varrock_teleport", 25), ("lumbridge_teleport", 31), ("falador_teleport", 37)];
pub const BONES_TO_BANANAS: &str = "bones_to_bananas";

/// Runes of a spell from the dump (case-insensitive debugname).
pub fn spell_runes(spell: &str) -> Vec<(String, i64)> {
    knowledge::spells::by_name(spell).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Enchant row for a strung (or unstrung) amulet: spell, level, product.
pub fn enchant_for(amulet: &str) -> Option<(&'static str, i64, &'static str)> {
    let strung = amulet.strip_prefix("unstrung_").map(|rest| format!("strung_{rest}")).unwrap_or_else(|| amulet.to_string());
    ENCHANTS.iter().find(|(_, _, input, _)| input.eq_ignore_ascii_case(&strung)).map(|(spell, level, _, product)| (*spell, *level, *product))
}

/// Required level of a teleport spell (`None` for anything else).
pub fn teleport_level(name: &str) -> Option<i64> {
    TELEPORTS.iter().find(|(spell, _)| spell.eq_ignore_ascii_case(name)).map(|(_, level)| *level)
}

/// Cast in the open session: `enchant_teleport <spell|amulet> [count]`.
/// Resolves spell, level, runes and target, then fails loud: no
/// spell-cast op in the SDK (dette P2.4).
pub fn run_enchant_teleport(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("enchant_teleport", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let _ = crate::kit::cycle::resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, MAGIC_STAT, 1);
    let wanted = resource.as_deref().ok_or_else(|| ScriptError::Usage(format!("enchant_teleport <spell|amulet> [count], nothing named at {:?}", bot.position())))?;
    let (spell, need, target) = if let Some((spell, need, product)) = enchant_for(wanted) {
        (spell.to_string(), need, product.to_string())
    } else if let Some(need) = teleport_level(wanted) {
        (wanted.to_string(), need, String::new())
    } else if wanted.eq_ignore_ascii_case(BONES_TO_BANANAS) {
        (BONES_TO_BANANAS.to_string(), 15, "bones".to_string())
    } else {
        return Err(ScriptError::Usage(format!("no F2P enchant, teleport or {BONES_TO_BANANAS} named {wanted:?} at {:?}", bot.position())));
    };
    if level < need {
        return Err(ScriptError::Usage(format!("{spell} needs level {need} at {:?}", bot.position())));
    }
    let runes = spell_runes(&spell);
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{spell} has no runes in the dump at {:?}", bot.position())));
    }
    if !target.is_empty() {
        let _ = obj_id(bot, &target)?;
    }
    bot.log(&format!("{spell} with {runes:?}"));
    Err(ScriptError::Usage(format!("spellcasting needs a spell-cast op in the SDK (dette P2.4): cast {spell} with {runes:?} at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{enchant_for, spell_runes, teleport_level, BONES_TO_BANANAS, ENCHANTS, TELEPORTS};

    /// Catalogue §5.10 ladder matches the spell dump: levels, cosmic
    /// rune counts and F2P flags.
    #[test]
    fn enchant_ladder_matches_the_spell_dump() {
        assert_eq!(ENCHANTS.map(|(_, level, _, _)| level), [7, 27, 49, 57]);
        for (spell, level, _, _) in ENCHANTS {
            let rows = lifescape_gamedata::spells::by_name(spell);
            assert_eq!(rows.len(), 1, "{spell} resolves once");
            assert_eq!(rows[0].level, level, "{spell} level");
            assert!(!rows[0].members, "{spell} F2P");
            assert!(spell_runes(spell).iter().any(|(name, _)| name == "cosmicrune"), "{spell} eats cosmic, got {:?}", spell_runes(spell));
        }
        assert_eq!(enchant_for("strung_sapphire_amulet"), Some(("enchant_lvl1", 7, "amulet_of_magic")));
        assert_eq!(enchant_for("unstrung_emerald_amulet"), Some(("enchant_lvl2", 27, "amulet_of_defence")), "unstrung maps too");
        assert!(enchant_for("strung_gold_amulet").is_none(), "gold never enchants");
        assert!(enchant_for("coins").is_none(), "junk never enchants");
    }

    /// Teleports and bones to bananas: levels and runes from the dump.
    #[test]
    fn teleports_and_bones_match_the_dump() {
        assert_eq!(TELEPORTS.map(|(_, level)| level), [25, 31, 37]);
        assert_eq!(teleport_level("varrock_teleport"), Some(25));
        assert_eq!(teleport_level("FALADOR_TELEPORT"), Some(37), "case-insensitive");
        assert_eq!(teleport_level("camelot_teleport"), None, "members never teleports");
        assert!(spell_runes("varrock_teleport").contains(&("lawrune".to_string(), 1)), "teleports eat law");
        let bones = lifescape_gamedata::spells::by_name(BONES_TO_BANANAS);
        assert_eq!(bones.len(), 1);
        assert_eq!((bones[0].level, bones[0].members), (15, false));
        let mut bones_runes = spell_runes(BONES_TO_BANANAS);
        bones_runes.sort();
        assert_eq!(bones_runes, vec![("earthrune".to_string(), 2), ("naturerune".to_string(), 1), ("waterrune".to_string(), 2)]);
    }

    /// Every enchant product exists in the object dump.
    #[test]
    fn enchant_products_exist() {
        for (_, _, input, product) in ENCHANTS {
            assert!(!lifescape_gamedata::objs_by_name(input).is_empty(), "{input} exists");
            assert!(!lifescape_gamedata::objs_by_name(product).is_empty(), "{product} exists");
        }
    }
}
