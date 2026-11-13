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

/// F2P elemental staves and the rune each stands in for while worn
/// (server `magic_staff.dbrow`, `magic.rs2` rune check).
pub const STAVES: [(&str, &str); 4] =
    [("staff_of_air", "airrune"), ("staff_of_water", "waterrune"), ("staff_of_earth", "earthrune"), ("staff_of_fire", "firerune")];

/// True when a worn staff pays `rune` (unlimited, nothing consumed).
pub fn staff_pays(bot: &Bot, rune: &str) -> bool {
    STAVES.iter().filter(|(_, paid)| *paid == rune).any(|(staff, _)| obj_id(bot, staff).is_ok_and(|obj| crate::kit::pack::is_worn(bot, obj)))
}

/// Runes of a cast the pack cannot fund, a worn staff's element excluded.
pub fn spell_shortfall(bot: &Bot, runes: &[(String, i64)]) -> Vec<String> {
    let unpaid: Vec<(String, i64)> = runes.iter().filter(|(name, _)| !staff_pays(bot, name)).cloned().collect();
    missing_runes(bot, &unpaid)
}

/// Wield a carried staff of one of the cast's elements when no worn
/// staff pays any of them yet (the planner sized the runes counting it).
/// True when a staff went on.
pub fn wield_staff_for(bot: &mut Bot, runes: &[(String, i64)]) -> Result<bool, ScriptError> {
    if runes.iter().any(|(name, _)| staff_pays(bot, name)) {
        return Ok(false);
    }
    for (staff, rune) in STAVES {
        if !runes.iter().any(|(name, _)| name == rune) {
            continue;
        }
        let Ok(obj) = obj_id(bot, staff) else { continue };
        if pack_total(bot, obj) > 0 && crate::kit::gear::wield_held(bot, obj)? {
            bot.log(&format!("wielding {staff} for the {rune} of the cast"));
            return Ok(true);
        }
    }
    Ok(false)
}

/// Cast on a carried item: resolve pack slot + com, send `OPHELDT`.
/// The caller pumps to its own observable (coins, product, removal).
pub fn cast_on_held(bot: &mut Bot, spell: &str, obj: u16) -> Result<(), ScriptError> {
    let spell_com = spell_com(bot, spell)?;
    let Some(pack_com) = bot.backpack().0 else {
        return Err(ScriptError::Failed(format!("pack not streamed at {:?}", bot.position())));
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

    /// The ordered spell's shortfall: a worn fire staff pays fire strike's
    /// fire runes, the mind and air runes stay owed by name; the planner's
    /// spell is cast exactly (the strike blocks on ITS runes).
    #[test]
    fn worn_staff_pays_its_element_only() {
        let mut bot = offline_bot();
        let runes = crate::activities::magic_style::spell_runes("fire_strike");
        let mut owed = super::spell_shortfall(&bot, &runes);
        owed.sort();
        assert_eq!(owed, vec!["airrune".to_string(), "firerune".to_string(), "mindrune".to_string()]);
        let staff = obj_id(&bot, "staff_of_fire").expect("fire staff in the dumps");
        let worn_com = (bot.tables.com_id)(crate::kit::gear::WORN_COM_NAME).expect("worn com");
        bot.world.inv.insert(worn_com, [(3u8, (staff, 1u32))].into_iter().collect());
        assert!(super::staff_pays(&bot, "firerune") && !super::staff_pays(&bot, "airrune"));
        let mut owed = super::spell_shortfall(&bot, &runes);
        owed.sort();
        assert_eq!(owed, vec!["airrune".to_string(), "mindrune".to_string()], "fire paid by the staff");
        bot.combat_style = "magic".to_string();
        bot.combat_spell = Some("fire_strike".to_string());
        match crate::kit::strike::attack_npc(&mut bot, 1, true) {
            Err(crate::ScriptError::Blocked { mut missing }) => {
                missing.sort();
                assert_eq!(missing, vec!["airrune".to_string(), "mindrune".to_string()]);
            }
            other => panic!("blocked on the ordered spell's runes, got {other:?}"),
        }
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
