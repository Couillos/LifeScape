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

use crate::kit::magic::{cast_on_held, missing_runes};
use crate::kit::pack::{obj_id, pack_total};
use crate::kit::survival::{check_events, learn_level, wait_pack_streamed};
use crate::{ScriptError, ScriptResult};

pub const MAGIC_STAT: u8 = 6;
pub const LOW_SPELL: &str = "lowlvl_alchemy";
pub const HIGH_SPELL: &str = "highlvl_alchemy";
/// Ticks to wait out one cast (server delays plus the consume tick).
pub const CAST_TICKS: u64 = 40;

/// Runes of a spell from the dump (case-insensitive debugname).
pub fn spell_runes(spell: &str) -> Vec<(String, i64)> {
    knowledge::spells::by_name(spell).into_iter().flat_map(|entry| entry.runes).collect()
}

/// Alch values of `name` from the dump (`None` when it never alchs).
pub fn alch_value(name: &str) -> Option<(i64, i64)> {
    knowledge::objs_by_name(name).into_iter().find(|entry| entry.alch_high > 0).map(|entry| (entry.alch_high, entry.alch_low))
}

/// Alch a banked item in the open session: `alchemy <item> [count]`
/// (low below 55, high from 55; count omitted or 0 = the pack). Missing
/// runes or items fail `Blocked` (the brain files obtain tasks); one
/// cast consumes the item and pays ~60% of its value in coins.
pub fn run_alchemy(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (resource, rest) = crate::kit::cycle::split_resource(args);
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
        return Err(crate::kit::cycle::blocked_level("magic", i64::from(need)));
    }
    let item = obj_id(bot, item_name)?;
    let coins = obj_id(bot, "coins")?;
    let runes = spell_runes(spell);
    if runes.is_empty() {
        return Err(ScriptError::Usage(format!("{spell} has no runes in the dump at {:?}", bot.position())));
    }
    let (worth_high, worth_low) = alch_value(item_name).ok_or_else(|| ScriptError::Usage(format!("{item_name} never alchs at {:?}", bot.position())))?;
    let worth = if high { worth_high } else { worth_low };
    let missing = missing_runes(bot, &runes);
    if !missing.is_empty() {
        return Err(ScriptError::Blocked { missing });
    }
    if pack_total(bot, item) == 0 {
        return Err(ScriptError::Blocked { missing: vec![item_name.to_string()] });
    }
    let want = spec.targets.count.filter(|count| *count > 0).unwrap_or(u32::MAX);
    // The order's other stops (`--level magic N`, budget) end it too.
    let mut spec = spec;
    spec.targets.count = None;
    let state = crate::kit::cycle::CycleState::begin(bot, &spec);
    let mut done = 0u32;
    while done < want {
        if let Some(stop) = crate::kit::cycle::poll_stop(bot, &state, &spec) {
            bot.log(&format!("ok {spell} x{done} on {item_name}, order met"));
            return crate::kit::cycle::exit_code(stop);
        }
        if pack_total(bot, item) == 0 || !missing_runes(bot, &runes).is_empty() {
            break;
        }
        alch_one(bot, spell, item, coins)?;
        done += 1;
    }
    bot.log(&format!("ok {spell} x{done} on {item_name} (~{worth} coins each)"));
    Ok(0)
}

/// One cast: the item leaves (or the coins grow) within the budget.
fn alch_one(bot: &mut Bot, spell: &str, item: u16, coins: u16) -> Result<(), ScriptError> {
    let had_item = pack_total(bot, item);
    let had_coins = pack_total(bot, coins);
    cast_on_held(bot, spell, item)?;
    let target = bot.world.ticks + CAST_TICKS;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if pack_total(bot, item) < had_item || pack_total(bot, coins) > had_coins {
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{spell} never landed at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{alch_value, run_alchemy, spell_runes, HIGH_SPELL, LOW_SPELL, MAGIC_STAT};
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

    /// Runes starve loud: an unfunded pack blocks with the rune names
    /// (the brain files them as obtain tasks), never a blind cast.
    #[test]
    fn unfunded_pack_blocks_on_runes() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (21, 0));
        fund_pack(&mut bot, &[(0, "yew_logs", 1)]);
        let error = run_alchemy(&mut bot, &["yew_logs".to_string()]).expect_err("no runes, no cast");
        assert!(matches!(error, crate::ScriptError::Blocked { .. }), "blocked, got {error}");
        assert!(format!("{error}").contains("naturerune"), "names the rune, got {error}");
    }

    /// A funded cast reaches the send brick: dead socket fails `Sdk`,
    /// never the old dette `Usage`.
    #[test]
    fn funded_cast_reaches_the_send_brick() {
        let mut bot = offline_bot();
        bot.world.stats.insert(MAGIC_STAT, (21, 0));
        fund_pack(&mut bot, &[(0, "yew_logs", 1), (1, "naturerune", 1), (2, "firerune", 3)]);
        let logs = obj_id(&bot, "yew_logs").expect("logs resolve");
        let error = run_alchemy(&mut bot, &["yew_logs".to_string()]).expect_err("dead socket fails");
        assert!(matches!(error, crate::ScriptError::Sdk(_)), "send path, got {error}");
        assert_eq!(pack_total(&bot, logs), 1, "nothing consumed offline");
    }

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
