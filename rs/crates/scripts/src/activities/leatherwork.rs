//! Leatherwork: needle and thread on tanned leather, inventory only
//! (catalogue §5.6). Hides reach the tanner NPC first (service, coins);
//! the tanner grid mapping is unverified, so tanning stays a loud dette
//! instead of a blind press. The craft answers like gem cutting.
//!
//! Process: poste tanneur (service) puis inventaire · action coudre ·
//! interface choix/compteur · intrants cuir + aiguille + fil · produits
//! pièces de cuir.

#![forbid(unsafe_code)]

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::produce::{count_answer, match_option};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::skill_walk::walk_checked;
use crate::{ScriptError, ScriptResult};

pub const CRAFTING_STAT: u8 = 12;
pub const NEEDLE: &str = "needle";
pub const THREAD: &str = "thread";
pub const HIDE: &str = "cow_hide";
pub const LEATHER: &str = "leather";
/// Tanning NPC family: `kit::tan` owns the grid (verified buttons).
pub use crate::kit::tan::TANNERS;
pub const DEFAULT_PRODUCT: &str = "leather_gloves";
const ITEM_TICKS: u64 = 40;

/// F2P leather recipe making `product` (members excluded).
pub fn leather_recipe(product: &str) -> Option<knowledge::recipes::Recipe> {
    knowledge::recipes::making(product, Some("leather")).into_iter().find(|entry| entry.members == Some(false))
}

/// Sew banked loads in the open session: `leatherwork [product] [count]`
/// (gloves when omitted, 0 = forever). Hides without leather tan at Ellis
/// first (verified `tanner` grid in `kit::tan`); the sew uses the verified
/// `leather_crafting` x1 buttons, chat options only if they speak.
pub fn run_leatherwork(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("leatherwork", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, CRAFTING_STAT, 1);
    let product_name = resource.as_deref().unwrap_or(DEFAULT_PRODUCT);
    let recipe = leather_recipe(product_name).ok_or_else(|| ScriptError::Usage(format!("no F2P leather recipe makes {product_name:?} at {:?}", bot.position())))?;
    if level < recipe.level {
        return Err(ScriptError::Usage(format!("sewing {product_name} needs level {} at {:?}", recipe.level, bot.position())));
    }
    let leather = obj_id(bot, LEATHER)?;
    let needle = obj_id(bot, NEEDLE)?;
    let thread = obj_id(bot, THREAD)?;
    let _ = obj_id(bot, product_name)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_sew: Vec<u16> = vec![needle, thread, leather];
    keep_sew.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_sew, 0)?;
    fill_sew(bot, &bank, leather, thread)?;
    if pack_total(bot, leather) == 0 {
        // The withdraw inside names the real missing (hide or coins);
        // no `bank_items` read here: the stream is empty after close.
        tan_from_bank(bot, &bank, &keep_sew, product_name, leather, thread)?;
        if pack_total(bot, leather) == 0 {
            return Err(ScriptError::Blocked { missing: vec![LEATHER.to_string()] });
        }
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, leather) == 0 || pack_total(step_bot, thread) == 0 {
            bank_trip(step_bot, &bank, &keep_sew, 0)?;
            fill_sew(step_bot, &bank, leather, thread)?;
            if pack_total(step_bot, leather) == 0 {
                return Err(ScriptError::Blocked { missing: vec![LEATHER.to_string()] });
            }
            if pack_total(step_bot, thread) == 0 {
                return Err(ScriptError::Blocked { missing: vec![THREAD.to_string()] });
            }
        }
        let before = pack_total(step_bot, leather);
        while pack_total(step_bot, leather) > 0 && pack_total(step_bot, thread) > 0 {
            match sew_one(step_bot, leather, needle, product_name) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died sewing, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(()) => {}
            }
        }
        let made = before.saturating_sub(pack_total(step_bot, leather));
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} sewed {made} {product_name} (crafting={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Tan a pack of banked hides at Ellis, then come back and reload the
/// sew kit: withdraw hides + fare, walk, tan, walk back, redeposit the
/// leftovers and top up leather + thread for the loop.
fn tan_from_bank(
    bot: &mut Bot,
    bank: &lifescape_gamedata::banks::Bank,
    keep_sew: &[u16],
    product: &str,
    leather: u16,
    thread: u16,
) -> Result<(), ScriptError> {
    let Some((hide_name, _, cost, _)) = crate::kit::tan::tan_spec(product) else {
        return Err(ScriptError::Blocked { missing: vec![LEATHER.to_string()] });
    };
    let hide = obj_id(bot, hide_name)?;
    let coins = obj_id(bot, "coins")?;
    let free = free_slots(bot) as u32;
    bank_withdraw_items(bot, bank, &[hide], free.saturating_sub(1))?;
    let held = pack_total(bot, hide);
    // No stale bank-stream read: the withdraw above is the verdict. Zero
    // hides withdrawn means none banked (the old `bank_items` check after
    // `close_modal` read an empty stream and skipped the tan).
    if held == 0 {
        return Err(ScriptError::Blocked { missing: vec![hide_name.to_string()] });
    }
    bank_withdraw_items(bot, bank, &[coins], held.saturating_mul(cost))?;
    crate::kit::tan::walk_to_tanner(bot)?;
    let tanned = crate::kit::tan::tan_run(bot, product)?;
    bot.log(&format!("ok tanned {tanned} {hide_name}"));
    if !walk_checked(bot, bank.x as i32, bank.z as i32, crate::kit::tan::TANNER_WALK_TICKS)? {
        return Err(ScriptError::Usage(format!("no walk back to the bank at {:?}", bot.position())));
    }
    bank_trip(bot, bank, keep_sew, 0)?;
    fill_sew(bot, bank, leather, thread)?;
    Ok(())
}

/// Top the pack up with banked leather and thread (nothing when the bank
/// holds none or the booth is far: the caller reports `blocked`).
fn fill_sew(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, leather: u16, thread: u16) -> Result<(), ScriptError> {
    let free = free_slots(bot) as u32;
    if free <= 1 || (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[leather], free - 1)?;
    bank_withdraw_items(bot, bank, &[thread], 1)?;
    Ok(())
}

/// x1 button of the verified `leather_crafting` grid (`tanner.rs2` style
/// mapping from `content/scripts/.../leather/leather.rs2`): gloves 116,
/// boots 119, vambraces 122, chaps 125, cowl 131, armour 113. Hard leather
/// sews direct (no modal), so it maps to nothing here.
fn sew_button(product_name: &str) -> Option<&'static str> {
    match product_name.to_lowercase().as_str() {
        "leather_gloves" => Some("leather_crafting:com_116"),
        "leather_boots" => Some("leather_crafting:com_119"),
        "leather_vambraces" => Some("leather_crafting:com_122"),
        "leather_chaps" => Some("leather_crafting:com_125"),
        "leather_cowl" => Some("leather_crafting:com_131"),
        "leather_armour" => Some("leather_crafting:com_113"),
        _ => None,
    }
}

/// Sew one piece: click, answer the choice or counter, wait out the
/// consumption. `op_held_u` opens the `leather_crafting` grid live (see
/// `leather.rs2`), never chat options: when options stay silent but the
/// grid modal is open, press the verified x1 button instead.
fn sew_one(bot: &mut Bot, leather: u16, needle: u16, product_name: &str) -> Result<(), ScriptError> {
    let had = pack_total(bot, leather);
    let (com, slot) = inv_slot(bot, leather)?;
    let (use_com, use_slot) = inv_slot(bot, needle)?;
    bot.op_held_u(leather, slot, com, needle, use_slot, use_com)?;
    let grid: Option<u16> = (bot.tables.com_id)("leather_crafting");
    let button: Option<u16> = sew_button(product_name).and_then(|name| (bot.tables.com_id)(name));
    let mut pressed = false;
    let target = bot.world.ticks + ITEM_TICKS;
    while bot.world.ticks < target {
        if !bot.chat_options().is_empty() {
            if match_option(&bot.chat_options(), product_name).is_none() {
                return Err(ScriptError::Usage(format!("sew {product_name:?} matches no option at {:?}", bot.position())));
            }
            bot.chat_choose(product_name)?;
        } else if !pressed
            && grid.is_some_and(|grid| {
                bot.world.modal.as_ref().is_some_and(|modal| modal.interface_id == grid)
            })
        {
            let Some(com) = button else {
                return Err(ScriptError::Usage(format!("sew {product_name:?} has no verified grid button at {:?}", bot.position())));
            };
            bot.if_button(com)?;
            pressed = true;
        }
        if bot.world.count_dialog {
            bot.resume_pcount(count_answer(had, 0))?;
        }
        bot.pump(2)?;
        if pack_total(bot, leather) < had {
            return Ok(());
        }
    }
    Err(ScriptError::Usage(format!("{product_name} never sewed at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{leather_recipe, DEFAULT_PRODUCT, HIDE, LEATHER, NEEDLE, TANNERS, THREAD};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.6 F2P ladder comes from the dump (gloves 1 up to
    /// hardleather body 28, dragon rows excluded as members).
    #[test]
    fn leather_ladder_comes_from_gamedata() {
        for (product, level) in [("leather_gloves", 1), ("leather_boots", 7), ("leather_cowl", 9), ("leather_vambraces", 11), ("leather_armour", 14), ("leather_chaps", 18), ("hardleather_body", 28)] {
            let recipe = leather_recipe(product).unwrap_or_else(|| panic!("{product} recipe resolves"));
            assert_eq!(recipe.kind, "leather");
            assert_eq!(recipe.level, level, "{product} level");
            assert_eq!(recipe.members, Some(false), "{product} F2P");
            assert!(recipe.inputs.iter().any(|(name, _)| name == "leather" || name == "hard_leather"), "{product} eats leather, got {:?}", recipe.inputs);
        }
        assert!(leather_recipe("coif").is_none(), "members coif excluded");
        assert!(leather_recipe("LEATHER_GLOVES").is_some(), "case-insensitive");
        assert!(leather_recipe(DEFAULT_PRODUCT).is_some(), "default product resolves");
    }

    /// Tools, hide, leather and tanners all resolve through live tables.
    #[test]
    fn tanner_inputs_resolve() {
        let bot = offline_bot();
        for name in [NEEDLE, THREAD, HIDE, LEATHER, "hard_leather"] {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
        let npcs = lifescape_gamedata::npcs_by_name(TANNERS[0]);
        assert!(!npcs.is_empty(), "a tanner NPC exists");
    }

    /// Sew buttons come from `leather.rs2` (x1 of each F2P product);
    /// hard leather sews direct, so it maps to nothing.
    #[test]
    fn sew_buttons_match_the_server_script() {
        for (product, com) in [
            ("leather_gloves", "leather_crafting:com_116"),
            ("leather_boots", "leather_crafting:com_119"),
            ("leather_vambraces", "leather_crafting:com_122"),
            ("leather_chaps", "leather_crafting:com_125"),
            ("leather_cowl", "leather_crafting:com_131"),
            ("leather_armour", "leather_crafting:com_113"),
        ] {
            assert_eq!(super::sew_button(product), Some(com), "{product} button");
            let bot = offline_bot();
            assert!((bot.tables.com_id)(com).is_some(), "{com} resolves");
        }
        assert_eq!(super::sew_button("hardleather_body"), None, "hard sews direct");
        assert_eq!(super::sew_button("LEATHER_GLOVES"), Some("leather_crafting:com_116"), "case-insensitive");
    }
}
