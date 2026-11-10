//! Magic driver: spellbook casts outside combat (kit/ frozen wording
//! aside, this is the brick the P2.4 dette waited for). Spell buttons
//! are `magic:<debugname>` comps; the server routes `OPHELDT` (carried
//! item: alchemy, superheat, enchant, bananas), `OPOBJT` (ground item:
//! telegrab) and `IF_BUTTON` (teleports) by that com.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_slot, pack_total};
use crate::ScriptError;

/// Spellbook button com for a spell debugname (`magic:lowlvl_alchemy`).
/// Unknown names fail loud: a bad com would miscast live.
pub fn spell_com(bot: &Bot, spell: &str) -> Result<u16, ScriptError> {
    (bot.tables.com_id)(&format!("magic:{spell}"))
        .ok_or_else(|| ScriptError::Usage(format!("no spell button magic:{spell} at {:?}", bot.position())))
}

/// Runes of `runes` missing from the pack (names the brain files as
/// obtain tasks), empty when the cast is funded.
pub fn missing_runes(bot: &Bot, runes: &[(String, i64)]) -> Vec<String> {
    let mut missing = Vec::new();
    for (name, count) in runes {
        let Ok(obj) = obj_id(bot, name) else {
            missing.push(name.clone());
            continue;
        };
        if pack_total(bot, obj) < u32::try_from(*count).unwrap_or(u32::MAX) {
            missing.push(name.clone());
        }
    }
    missing
}

/// Cast on a carried item: resolve pack slot + com, send `OPHELDT`.
/// The caller pumps to its own observable (coins, product, removal).
pub fn cast_on_held(bot: &mut Bot, spell: &str, obj: u16) -> Result<(), ScriptError> {
    let spell_com = spell_com(bot, spell)?;
    let Some(pack_com) = bot.backpack().0 else {
        return Err(ScriptError::Usage(format!("pack not streamed at {:?}", bot.position())));
    };
    let slot = pack_slot(bot, obj)?;
    bot.op_held_t(obj, slot, pack_com, spell_com)?;
    Ok(())
}

/// Cast on a ground item: send `OPOBJT` at its tile. The caller pumps
/// to its own observable (the item leaving the ground).
pub fn cast_on_ground(bot: &mut Bot, spell: &str, x: i32, z: i32, obj: u16) -> Result<(), ScriptError> {
    let spell_com = spell_com(bot, spell)?;
    bot.op_obj_t(x, z, obj, spell_com)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{cast_on_ground, cast_on_held, missing_runes, spell_com};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Every dette spell has a spellbook button com in the dumps.
    #[test]
    fn every_dette_spell_has_a_button() {
        let bot = offline_bot();
        for spell in [
            "telekinetic_grab",
            "lowlvl_alchemy",
            "highlvl_alchemy",
            "superheat_item",
            "enchant_lvl1",
            "bones_to_bananas",
            "varrock_teleport",
        ] {
            assert!(spell_com(&bot, spell).is_ok(), "{spell} resolves a button");
        }
        assert!(spell_com(&bot, "no_such_spell").is_err(), "unknown spell fails loud");
    }

    /// Empty pack misses every rune; the funded case needs no top-up.
    #[test]
    fn shortfall_names_missing_runes() {
        let bot = offline_bot();
        let runes = vec![("airrune".to_string(), 1), ("lawrune".to_string(), 1)];
        assert_eq!(missing_runes(&bot, &runes), runes.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>());
        assert!(missing_runes(&bot, &[]).is_empty(), "no runes, no shortfall");
    }

    /// Casts reach the send path: dead socket fails `Sdk(Login)`, never
    /// `Usage` — the brick is wired, only the server is missing.
    #[test]
    fn casts_reach_the_send_path_offline() {
        let mut bot = offline_bot();
        let error = cast_on_ground(&mut bot, "telekinetic_grab", 2930, 3515, 0x10).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        let air = obj_id(&bot, "airrune").expect("airrune resolves");
        let com = (bot.tables.com_id)("inventory:inv").expect("pack com in dumps");
        bot.world.inv.insert(com, [(0u8, (air, 1u32))].into_iter().collect());
        bot.world.inv_cap.insert(com, 28);
        let error = cast_on_held(&mut bot, "lowlvl_alchemy", air).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
    }
}
