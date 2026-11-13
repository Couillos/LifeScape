//! Uniform: the brain's `--wear <item>` (args contract 2026-11-13). A
//! guild site (Crafting Guild: brown apron, Cooks' Guild: chef's hat) is
//! behind a door that checks the WORN item (`crafting_guild.rs2`,
//! `cooking_guild.rs2`); the nav plans through it only with `worn:<id>`.
//! The run wears it before any walk: from the pack, else withdrawn from
//! the bank (nearest the site), else `Blocked{<item>}` so the brain files
//! the obtain. Never a substitute, never a guild-less fallback.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::gear::wield_held;
use crate::kit::pack::{is_worn, obj_id, pack_total};
use crate::ScriptError;

/// Wear every uniform piece the order names. No piece named: no-op (no
/// world access). Unknown names fail `Usage` (a contract bug); a piece
/// neither carried nor banked blocks by name.
pub fn wear_uniform(bot: &mut Bot, items: &[String]) -> Result<(), ScriptError> {
    for name in items {
        let obj = obj_id(bot, name)?;
        if is_worn(bot, obj) {
            continue;
        }
        if pack_total(bot, obj) == 0 {
            let bank = match crate::kit::site::site_bank(bot) {
                Some(bank) => bank,
                None => crate::kit::bank::nearest_bank(bot)?,
            };
            bot.log(&format!("uniform {name} not carried, fetching it from the bank at ({},{})", bank.x, bank.z));
            crate::kit::withdraw::bank_withdraw_items(bot, &bank, &[obj], 1)?;
        }
        if pack_total(bot, obj) == 0 {
            return Err(ScriptError::Blocked { missing: vec![name.clone()] });
        }
        if !wield_held(bot, obj)? || !is_worn(bot, obj) {
            return Err(ScriptError::Failed(format!("could not wear the uniform {name} at {:?}", bot.position())));
        }
        bot.log(&format!("wearing {name} for the guild door"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::wear_uniform;
    use crate::kit::gear::WORN_COM_NAME;
    use crate::kit::pack::{is_worn, obj_id};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// No uniform ordered: nothing touches the world; one already worn
    /// stays put; an unknown name is a contract bug (`Usage`).
    #[test]
    fn worn_or_unordered_uniforms_need_nothing() {
        let mut bot = offline_bot();
        assert!(wear_uniform(&mut bot, &[]).is_ok());
        let hat = obj_id(&bot, "chefs_hat").expect("chef's hat in the dumps");
        let worn_com = (bot.tables.com_id)(WORN_COM_NAME).expect("worn com in the dumps");
        bot.world.inv.insert(worn_com, [(0u8, (hat, 1u32))].into_iter().collect());
        assert!(is_worn(&bot, hat));
        assert!(wear_uniform(&mut bot, &["chefs_hat".to_string()]).is_ok(), "already worn, no walk");
        assert!(matches!(wear_uniform(&mut bot, &["no_such_apron".to_string()]), Err(crate::ScriptError::Usage(_))));
    }
}
