//! Amulet stringing: ball of wool on unstrung amulets, inventory only
//! (catalogue §5.6). Direct interface: one click strings one amulet, no
//! dialogue to answer. Any chat option at all is loud (no guess).
//!
//! Process: poste aucun (inventaire) · action enfiler · interface
//! directe · intrants amulette non enfilée + pelote · produits amulette
//! enfilée.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const BALL: &str = "ball_of_wool";
pub const STRING_LEVEL: i64 = 1;
/// F2P amulets (catalogue §5.6) and the silver holy symbol: unstrung
/// input and strung product (`stringing.rs2` strings `nostringstar` with
/// the same ball-of-wool use as the amulets).
pub const AMULETS: [(&str, &str); 6] = [
    ("unstrung_gold_amulet", "strung_gold_amulet"),
    ("unstrung_sapphire_amulet", "strung_sapphire_amulet"),
    ("unstrung_emerald_amulet", "strung_emerald_amulet"),
    ("unstrung_ruby_amulet", "strung_ruby_amulet"),
    ("unstrung_diamond_amulet", "strung_diamond_amulet"),
    ("nostringstar", "stringstar"),
];

/// Strung product of an unstrung amulet (`None` for anything else).
pub fn strung_for(unstrung: &str) -> Option<&'static str> {
    AMULETS.iter().find(|(input, _)| input.eq_ignore_ascii_case(unstrung)).map(|(_, strung)| *strung)
}

/// String banked loads in the open session: `stringing [unstrung] [count]`
/// (gold when omitted, 0 = forever).
pub fn run_stringing(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
    let spec = crate::kit::cycle::parse_cli("stringing", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    if level < STRING_LEVEL {
        return Err(crate::kit::cycle::blocked_level("fletching", i64::from(STRING_LEVEL)));
    }
    let unstrung_name = resource.as_deref().unwrap_or("unstrung_gold_amulet");
    let strung_name = strung_for(unstrung_name).ok_or_else(|| ScriptError::Usage(format!("no F2P stringing amulet named {unstrung_name:?} at {:?}", bot.position())))?;
    let unstrung = obj_id(bot, unstrung_name)?;
    let ball = obj_id(bot, BALL)?;
    let _ = obj_id(bot, strung_name)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_string: Vec<u16> = vec![ball, unstrung];
    keep_string.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_string, 0)?;
    fill_string(bot, &bank, unstrung, ball)?;
    if pack_total(bot, unstrung) == 0 {
        return Err(ScriptError::Blocked { missing: vec![unstrung_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, unstrung) == 0 || pack_total(step_bot, ball) == 0 {
            bank_trip(step_bot, &bank, &keep_string, 0)?;
            fill_string(step_bot, &bank, unstrung, ball)?;
            if pack_total(step_bot, unstrung) == 0 {
                return Err(ScriptError::Blocked { missing: vec![unstrung_name.to_string()] });
            }
            if pack_total(step_bot, ball) == 0 {
                return Err(ScriptError::Blocked { missing: vec![BALL.to_string()] });
            }
        }
        let before = pack_total(step_bot, unstrung);
        while pack_total(step_bot, unstrung) > 0 && pack_total(step_bot, ball) > 0 && !state.want_met(step_bot, &spec) {
            match string_one(step_bot, unstrung, ball, strung_name) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died stringing, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        let made = before.saturating_sub(pack_total(step_bot, unstrung));
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        Ok(state.load_done(step_bot, &format!("strung {made} {unstrung_name} (crafting={level})")))
    });
    outcome
}

/// Top the pack up with banked amulets and wool, one ball per amulet
/// (nothing when the bank holds none or the booth is far: the caller
/// reports `blocked`). Live admin 2026-11-15: one ball per load strung
/// one amulet per bank trip.
fn fill_string(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, unstrung: u16, ball: u16) -> Result<(), ScriptError> {
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    let (amulets, balls) = pair_load(free_slots(bot) as u32, pack_total(bot, unstrung), pack_total(bot, ball));
    if amulets > 0 {
        bank_withdraw_items(bot, bank, &[unstrung], amulets)?;
    }
    if balls > 0 {
        bank_withdraw_items(bot, bank, &[ball], balls)?;
    }
    // The bank closes before the first string: an open bank reads as an
    // unexpected interface (live admin 2026-11-15, "stringing opened an
    // interface" right after the withdrawal).
    if bot.world.modal.is_some() {
        bot.close_modal()?;
        bot.pump(2)?;
    }
    Ok(())
}

/// Amulets and balls to withdraw so the pack holds as many of each:
/// `free` slots, `amulets`/`balls` already carried.
fn pair_load(free: u32, amulets: u32, balls: u32) -> (u32, u32) {
    let pairs = (free + amulets + balls) / 2;
    (pairs.saturating_sub(amulets), pairs.saturating_sub(balls))
}

/// String one amulet: direct click, no dialogue. Any chat option or
/// counter is loud (an unexpected interface, never guessed).
fn string_one(bot: &mut Bot, unstrung: u16, ball: u16, strung_name: &str) -> Result<(), ScriptError> {
    let had = pack_total(bot, unstrung);
    let (com, slot) = inv_slot(bot, unstrung)?;
    let (use_com, use_slot) = inv_slot(bot, ball)?;
    bot.op_held_u(unstrung, slot, com, ball, use_slot, use_com)?;
    let target = bot.world.ticks + 40;
    while bot.world.ticks < target {
        if !bot.chat_options().is_empty() || bot.world.count_dialog || bot.world.modal.is_some() {
            return Err(ScriptError::Failed(format!("stringing {strung_name} opened an interface at {:?}", bot.position())));
        }
        bot.pump(2)?;
        if pack_total(bot, unstrung) < had {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{strung_name} never strung at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{strung_for, AMULETS, BALL, STRING_LEVEL};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6: five F2P amulets string at level 1.
    #[test]
    fn a_load_pairs_amulets_with_balls() {
        assert_eq!(super::pair_load(27, 0, 0), (13, 13));
        assert_eq!(super::pair_load(20, 2, 4), (11, 9));
        assert_eq!(super::pair_load(0, 5, 5), (0, 0));
    }

    /// The five amulets and the holy symbol (live admin 2026-11-15:
    /// `string:nostringstar` answered "no F2P stringing amulet").
    #[test]
    fn five_amulets_string_at_level_one() {
        assert_eq!(AMULETS.len(), 6);
        assert_eq!(strung_for("nostringstar"), Some("stringstar"));
        assert_eq!(STRING_LEVEL, 1);
        assert_eq!(strung_for("unstrung_gold_amulet"), Some("strung_gold_amulet"));
        assert_eq!(strung_for("UNSTRUNG_DIAMOND_AMULET"), Some("strung_diamond_amulet"), "case-insensitive");
        assert!(strung_for("strung_gold_amulet").is_none(), "already strung never strings");
        assert!(strung_for("coins").is_none(), "junk never strings");
    }

    /// Wool and every unstrung/strung pair resolve through live tables.
    #[test]
    fn wool_and_pairs_resolve() {
        let bot = offline_bot();
        assert!(obj_id(&bot, BALL).is_ok(), "ball of wool resolves");
        for (unstrung, strung) in AMULETS {
            assert!(obj_id(&bot, unstrung).is_ok(), "{unstrung} resolves");
            assert!(obj_id(&bot, strung).is_ok(), "{strung} resolves");
        }
    }
}
