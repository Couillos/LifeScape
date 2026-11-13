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

use crate::kit::magic::{cast_on_held, missing_runes, spell_com};
use crate::kit::pack::{obj_id, pack_total};
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

/// Strung input of an (un)strung amulet name, mirroring `enchant_for`.
fn strung_input(wanted: &str) -> String {
    wanted.strip_prefix("unstrung_").map(|rest| format!("strung_{rest}")).unwrap_or_else(|| wanted.to_string())
}

/// Required level of a teleport spell (`None` for anything else).
pub fn teleport_level(name: &str) -> Option<i64> {
    TELEPORTS.iter().find(|(spell, _)| spell.eq_ignore_ascii_case(name)).map(|(_, level)| *level)
}

/// Cast in the open session: `enchant_teleport <spell|amulet> [count]`.
/// Enchants and bananas cast on the carried target (`OPHELDT`);
/// teleports press the spellbook button (`IF_BUTTON`) and land the bot
/// elsewhere. Missing runes or targets fail `Blocked`.
pub fn run_enchant_teleport(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
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
    let (spell, need, input) = if let Some((spell, need, _)) = enchant_for(wanted) {
        (spell.to_string(), need, strung_input(wanted))
    } else if let Some(need) = teleport_level(wanted) {
        (wanted.to_string(), need, String::new())
    } else if wanted.eq_ignore_ascii_case(BONES_TO_BANANAS) {
        (BONES_TO_BANANAS.to_string(), 15, "bones".to_string())
    } else {
        return Err(ScriptError::Usage(format!("no F2P enchant, teleport or {BONES_TO_BANANAS} named {wanted:?} at {:?}", bot.position())));
    };
    if level < need {
        return Err(crate::kit::cycle::blocked_level("magic", i64::from(need)));
    }
    let runes = spell_runes(&spell);
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{spell} has no runes in the dump at {:?}", bot.position())));
    }
    let target_obj = if input.is_empty() { None } else { Some(obj_id(bot, &input)?) };
    let missing = missing_runes(bot, &runes);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    if target_obj.is_some_and(|obj| pack_total(bot, obj) == 0) {
        return Err(ScriptError::Blocked { missing: vec![input.clone()] });
    }
    // Teleports move the bot, not an item: exactly one cast per run.
    // Held targets run the pack (count omitted or 0 = the load).
    let want = if target_obj.is_none() { 1 } else { spec.targets.count.filter(|count| *count > 0).unwrap_or(u32::MAX) };
    // The order's other stops (`--want`, `--level magic N`, budget).
    let mut spec = spec;
    spec.targets.count = None;
    let state = crate::kit::cycle::CycleState::begin(bot, &spec);
    let mut done = 0u32;
    while done < want {
        if let Some(stop) = crate::kit::cycle::poll_stop(bot, &state, &spec).filter(|_| target_obj.is_some()) {
            bot.log(&format!("ok {spell} x{done}, order met"));
            return crate::kit::cycle::exit_code(stop);
        }
        if !missing_runes(bot, &runes).is_empty() {
            break;
        }
        if target_obj.is_some_and(|obj| pack_total(bot, obj) == 0) {
            break;
        }
        cast_one(bot, &spell, target_obj)?;
        done += 1;
    }
    bot.log(&format!("ok {spell} x{done}"));
    Ok(0)
}

/// Ticks to wait out one cast (server delays plus the convert tick).
pub const CAST_TICKS: u64 = 40;

/// One cast: held targets convert in the pack (input count moves),
/// teleports move the bot. Anything quiet past the budget is loud.
fn cast_one(bot: &mut Bot, spell: &str, target: Option<u16>) -> Result<(), ScriptError> {
    match target {
        Some(obj) => {
            let had = pack_total(bot, obj);
            cast_on_held(bot, spell, obj)?;
            let deadline = bot.world.ticks + CAST_TICKS;
            while bot.world.ticks < deadline {
                bot.pump(2)?;
                if pack_total(bot, obj) != had {
                    return Ok(());
                }
            }
            Err(ScriptError::Failed(format!("{spell} never landed at {:?}", bot.position())))
        }
        None => {
            let before = bot.position();
            let com = spell_com(bot, spell)?;
            bot.if_button(com)?;
            let deadline = bot.world.ticks + CAST_TICKS;
            while bot.world.ticks < deadline {
                bot.pump(2)?;
                if bot.position() != before {
                    return Ok(());
                }
            }
            Err(ScriptError::Failed(format!("{spell} never moved at {:?}", bot.position())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{enchant_for, run_enchant_teleport, spell_runes, teleport_level, BONES_TO_BANANAS, CAST_TICKS, ENCHANTS, MAGIC_STAT, TELEPORTS};
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
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        fund_pack(&mut bot, &[(0, "strung_sapphire_amulet", 1)]);
        let error = run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()])
            .expect_err("no runes, no cast");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got {error}");
        assert!(format!("{error}").contains("cosmicrune"), "names the rune, got {error}");
    }

    /// A funded enchant reaches the send brick: dead socket fails `Sdk`,
    /// never the old dette `Usage`.
    #[test]
    fn funded_enchant_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (7, 0));
        // cosmic + water for enchant_lvl1 (dump runes, sorted check below).
        let mut runes = spell_runes("enchant_lvl1");
        runes.sort();
        fund_pack(&mut bot, &[(0, "strung_sapphire_amulet", 1), (1, "cosmicrune", 1), (2, "waterrune", 1)]);
        let input = obj_id(&bot, "strung_sapphire_amulet").expect("input resolves");
        let error = run_enchant_teleport(&mut bot, &["strung_sapphire_amulet".to_string()])
            .expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        assert_eq!(pack_total(&bot, input), 1, "nothing converted offline");
        assert!(CAST_TICKS >= 3, "covers the server delays");
    }

    /// A funded teleport reaches the button press: dead socket fails
    /// `Sdk`, never the old dette `Usage`.
    #[test]
    fn funded_teleport_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (25, 0));
        fund_pack(&mut bot, &[(0, "airrune", 3), (1, "lawrune", 1), (2, "firerune", 1)]);
        let error = run_enchant_teleport(&mut bot, &["varrock_teleport".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
    }

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
