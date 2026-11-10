//! Gem cutting: chisel on uncut stones, inventory only (catalogue
//! §5.6). The click order is the proven inventory one (input on tool);
//! the choice dialogue answers with the cut name through the shared
//! adapter, the counter answers like produce. No station and no NPC, so
//! produce cannot run it (no inventory site: dette P2.4); the loop below
//! mirrors its vocabulary (blocked on no input, loud on bad input).
//!
//! Process: poste aucun (inventaire) · action tailler · interface
//! choix/compteur · intrants gemme brute + burin · produits gemme taillée.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{count_answer, match_option};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const CHISEL: &str = "chisel";
const ITEM_TICKS: u64 = 40;
/// F2P uncuts in tier order (catalogue §5.6): name and cutting level.
pub const UNCUT: [(&str, i64); 4] = [("uncut_sapphire", 20), ("uncut_emerald", 27), ("uncut_ruby", 34), ("uncut_diamond", 43)];

/// F2P cut recipe eating `name` (case-insensitive, members excluded).
pub fn gem_recipe(name: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::using(name, Some("gem")).into_iter().find(|entry| entry.members == Some(false))
}

/// Best cuttable uncut at `level` (highest tier unlocked, never above).
pub fn uncut_for_level(level: i64) -> &'static str {
    UNCUT.iter().rev().find(|(_, need)| level >= *need).map(|(name, _)| *name).unwrap_or("uncut_sapphire")
}

/// Cut banked loads in the open session: `cut_gems [uncut] [count]`
/// (best tier by level when omitted, 0 = forever).
pub fn run_cut_gems(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("cut_gems", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let uncut_name = resource.as_deref().unwrap_or_else(|| uncut_for_level(level));
    let recipe = gem_recipe(uncut_name).ok_or_else(|| ScriptError::Usage(format!("no F2P gem recipe eats {uncut_name:?} at {:?}", bot.position())))?;
    if level < recipe.level {
        return Err(ScriptError::Usage(format!("cutting {uncut_name} needs level {} at {:?}", recipe.level, bot.position())));
    }
    let cut_name = recipe.outputs.first().map(|(name, _)| name.clone()).unwrap_or_default();
    let uncut = obj_id(bot, uncut_name)?;
    let chisel = obj_id(bot, CHISEL)?;
    let _ = obj_id(bot, &cut_name)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_cut: Vec<u16> = vec![chisel, uncut];
    keep_cut.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_cut, 0)?;
    fill_uncut(bot, &bank, uncut)?;
    if pack_total(bot, uncut) == 0 {
        return Err(ScriptError::Blocked { missing: vec![uncut_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, uncut) == 0 {
            bank_trip(step_bot, &bank, &keep_cut, 0)?;
            fill_uncut(step_bot, &bank, uncut)?;
            if pack_total(step_bot, uncut) == 0 {
                return Err(ScriptError::Blocked { missing: vec![uncut_name.to_string()] });
            }
        }
        let before = pack_total(step_bot, uncut);
        while pack_total(step_bot, uncut) > 0 {
            match cut_one(step_bot, uncut, chisel, &cut_name) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died cutting, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        let made = before.saturating_sub(pack_total(step_bot, uncut));
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} cut {made} {uncut_name} (crafting={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Top the pack up with banked uncuts (nothing when the bank holds none
/// or the booth is far: the caller reports `blocked`, never an empty walk).
fn fill_uncut(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, uncut: u16) -> Result<(), ScriptError> {
    let free = free_slots(bot) as u32;
    if free == 0 {
        return Ok(());
    }
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[uncut], free)?;
    Ok(())
}

/// Cut one stone: click, answer the choice or counter through the shared
/// adapters, wait out the consumption. A grid modal is loud (its button
/// mapping is unverified: never press blind).
fn cut_one(bot: &mut Bot, uncut: u16, chisel: u16, cut_name: &str) -> Result<(), ScriptError> {
    let had = pack_total(bot, uncut);
    let (com, slot) = inv_slot(bot, uncut)?;
    let (use_com, use_slot) = inv_slot(bot, chisel)?;
    bot.op_held_u(uncut, slot, com, chisel, use_slot, use_com)?;
    let target = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < target {
        if !bot.chat_options().is_empty() {
            if match_option(&bot.chat_options(), cut_name).is_none() {
                return Err(ScriptError::Usage(format!("cut {cut_name:?} matches no option at {:?}", bot.position())));
            }
            bot.chat_choose(cut_name)?;
        }
        if bot.world.count_dialog {
            bot.resume_pcount(count_answer(had, 0))?;
        }
        bot.pump(2)?;
        if pack_total(bot, uncut) < had {
            return Ok(());
        }
    }
    Err(ScriptError::Usage(format!("{cut_name} never cut at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{gem_recipe, uncut_for_level, CHISEL, CRAFTING_STAT, UNCUT};
    use crate::kit::pack::obj_id;
    use crate::kit::produce::match_option;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6 tiers come from the dump: sapphire 20, emerald 27,
    /// ruby 34, diamond 43, each F2P with one cut output.
    #[test]
    fn gem_tiers_come_from_gamedata() {
        assert_eq!(UNCUT.map(|(_, level)| level), [20, 27, 34, 43]);
        for (uncut, level) in UNCUT {
            let recipe = gem_recipe(uncut).unwrap_or_else(|| panic!("{uncut} recipe resolves"));
            assert_eq!(recipe.kind, "gem");
            assert_eq!(recipe.level, level, "{uncut} level");
            assert_eq!(recipe.members, Some(false), "{uncut} F2P");
            assert_eq!(recipe.inputs, vec![(uncut.to_string(), 1)]);
            assert_eq!(recipe.outputs.len(), 1, "{uncut} cuts to one stone");
        }
        assert!(gem_recipe("uncut_opal").is_none(), "members opal excluded");
        assert!(gem_recipe("UNCUT_RUBY").is_some(), "case-insensitive");
        assert!(gem_recipe("coins").is_none(), "junk never cuts");
    }

    /// Tier pick: highest cuttable at the level, sapphire below 20.
    #[test]
    fn tier_pick_never_reaches_above() {
        assert_eq!(uncut_for_level(1), "uncut_sapphire");
        assert_eq!(uncut_for_level(27), "uncut_emerald");
        assert_eq!(uncut_for_level(99), "uncut_diamond");
    }

    /// Choice adapter picks the cut by name; chisel and stones resolve.
    #[test]
    fn choice_adapter_and_names_resolve() {
        let options = vec![(3u16, "Cut Sapphire".to_string()), (4, "Cut Emerald".to_string())];
        assert_eq!(match_option(&options, "sapphire"), Some(3));
        let bot = offline_bot();
        assert!(obj_id(&bot, CHISEL).is_ok(), "chisel resolves");
        for (uncut, _) in UNCUT {
            let recipe = gem_recipe(uncut).expect("recipe");
            let cut = recipe.outputs.first().expect("one output").0.clone();
            assert!(obj_id(&bot, &cut).is_ok(), "{cut} resolves");
        }
        assert_eq!(CRAFTING_STAT, 12, "provenance crafting id");
    }
}
