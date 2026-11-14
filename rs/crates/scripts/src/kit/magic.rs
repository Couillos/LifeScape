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

/// Runes of a cast pack + known bank cannot fund (worn staff element
/// excluded): empty when a bank trip refills the cast.
pub fn bank_shortfall(bot: &Bot, runes: &[(String, i64)]) -> Vec<String> {
    runes
        .iter()
        .filter(|(name, _)| !staff_pays(bot, name))
        .filter(|(name, count)| {
            let need = u32::try_from(*count).unwrap_or(u32::MAX);
            obj_id(bot, name).map(|obj| pack_total(bot, obj).saturating_add(crate::kit::pack::banked_usable(bot, obj)) < need).unwrap_or(true)
        })
        .map(|(name, _)| name.clone())
        .collect()
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

/// Ticks to wait, from the frame showing a cast's effect, before sending
/// the next `OPHELDT`. `p_delay(n)` keeps the caster busy until
/// `tick + 1 + n`, `OpHeldTHandler` silently drops a cast decoded while
/// delayed, and `World` decodes client input BEFORE it clears the delay
/// that tick: the next send must be decoded at `tick + 2 + n`, i.e. sent
/// `n + 1` frames after the effect. High alchemy `p_delay(3)` (a 5-tick
/// cycle), low alchemy, superheat and the enchants `p_delay(1)` (3),
/// bones to bananas `p_delay(2)`. Live 2026-11-14 (admin, 40 high alchs):
/// one send per cast.
pub fn cast_cooldown(spell: &str) -> u64 {
    match spell {
        "highlvl_alchemy" => 4,
        "bones_to_bananas" | "telekinetic_grab" => 3,
        _ => 2,
    }
}

/// Sends of one cast before it fails loud (a send swallowed by a server
/// delay or a lost frame is sent again, never waited out for 40 ticks).
pub const CAST_SENDS: u32 = 3;
/// Ticks one send gets to show its effect before the next send.
pub const CAST_LAND_TICKS: u64 = 8;

/// Paces casts on carried items: the next send waits out the server delay
/// the last landed cast left, a send that shows no effect within
/// `CAST_LAND_TICKS` is sent again (live 2026-11-14: the alchemy loop
/// recast the tick the coins landed, the server dropped it as delayed and
/// the run failed "never landed" 40 ticks later, one cast per run).
pub struct CastPacer {
    spell: String,
    ready_at: u64,
}

impl CastPacer {
    pub fn new(spell: &str) -> CastPacer {
        CastPacer { spell: spell.to_string(), ready_at: 0 }
    }

    /// Tick from which the server takes the next cast.
    pub fn ready_at(&self) -> u64 {
        self.ready_at
    }

    /// Note a cast that landed at `tick`.
    pub fn landed_at(&mut self, tick: u64) {
        self.ready_at = tick + cast_cooldown(&self.spell);
    }

    /// Wait out the last cast's server delay before any other click: a
    /// booth click sent while delayed is dropped like a cast (live
    /// 2026-11-14: the bank trip right after the 198th low alchemy was
    /// "refused from every stand").
    pub fn settle(&self, bot: &mut Bot) -> Result<(), ScriptError> {
        while bot.world.ticks < self.ready_at {
            bot.pump(1)?;
        }
        Ok(())
    }

    /// Cast on the carried `obj` until `landed` reads its effect: waits
    /// for the cooldown, sends, re-sends up to `CAST_SENDS`. Failed loud
    /// (with the last server line) when no send lands.
    pub fn cast_held(&mut self, bot: &mut Bot, obj: u16, landed: impl Fn(&Bot) -> bool) -> Result<(), ScriptError> {
        while bot.world.ticks < self.ready_at {
            bot.pump(1)?;
        }
        for _ in 0..CAST_SENDS {
            cast_on_held(bot, &self.spell, obj)?;
            let deadline = bot.world.ticks + CAST_LAND_TICKS;
            while bot.world.ticks < deadline {
                bot.pump(1)?;
                if landed(bot) {
                    self.landed_at(bot.world.ticks);
                    return Ok(());
                }
            }
        }
        let last = bot.world.messages.last().cloned().unwrap_or_default();
        Err(ScriptError::Failed(format!("{} never landed after {CAST_SENDS} sends at {:?} (last line {last:?})", self.spell, bot.position())))
    }
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
