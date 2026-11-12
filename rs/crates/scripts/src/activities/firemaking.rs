//! Firemaking activity: the inventory declaration (tinderbox on logs,
//! lit in a line on free tiles), run through the shared kit. One click
//! lights one log (proven order, logs on tinderbox, like the tutorial);
//! the loop steps aside after each fire so the line never stacks. Ashes
//! stay on the ground: the counted product is the burnt log.
//!
//! Process: poste aucun (cases libres, en ligne) · action allumer ·
//! interface directe · intrants bûches + tinderbox · produits cendres.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::bank::{bank_trip, bank_withdraw_items, nearest_bank};
use crate::kit::cycle::{resolve_keep, run, StepControl};
use crate::kit::pack::{free_slots, obj_id, pack_total};
use crate::kit::survival::{check_events, is_death_abort, learn_level, wait_pack_streamed};
use crate::quests::common::inv_slot;
use crate::{ScriptError, ScriptResult};

pub const FIREMAKING_STAT: u8 = 11;
pub const TINDERBOX: &str = "tinderbox";
/// F2P logs in tier order (catalogue §5.5): name and firemaking level.
pub const LOGS: [(&str, i64); 4] = [("logs", 1), ("oak_logs", 15), ("willow_logs", 30), ("yew_logs", 60)];

/// Required level to burn `name` (`None` when it is no F2P log).
pub fn log_level(name: &str) -> Option<i64> {
    LOGS.iter().find(|(logged, _)| logged.eq_ignore_ascii_case(name)).map(|(_, level)| *level)
}

/// Best burnable log at `level` (highest tier unlocked, never above).
pub fn best_log(level: i64) -> &'static str {
    LOGS.iter().rev().find(|(_, need)| level >= *need).map(|(name, _)| *name).unwrap_or("logs")
}

/// Burn banked loads in the open session: `firemaking [log] [count]`
/// (best tier by level when omitted, 0 = forever).
pub fn run_firemaking(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let resource = args.first().filter(|first| first.parse::<u32>().is_err() && first.as_str() != "toll" && first.as_str() != "wild" && !first.starts_with("--")).cloned();
    let rest: Vec<String> = if resource.is_some() { args[1..].to_vec() } else { args.to_vec() };
    let spec = crate::kit::cycle::parse_cli("firemaking", &rest)?;
    if spec.allow_paid {
        bot.allow_paid = true;
        bot.log("toll gates allowed for this run");
    }
    bot.allow_shop = spec.allow_shop;
    let task_keep = resolve_keep(bot, &spec)?;
    wait_pack_streamed(bot)?;
    check_events(bot)?;
    let level = learn_level(bot, FIREMAKING_STAT, 1);
    let log_name = resource.as_deref().unwrap_or_else(|| best_log(level));
    let need = log_level(log_name).ok_or_else(|| ScriptError::Usage(format!("no F2P firemaking log named {log_name:?} at {:?}", bot.position())))?;
    if level < need {
        return Err(ScriptError::Usage(format!("firemaking {log_name} needs level {need} at {:?}", bot.position())));
    }
    let log = obj_id(bot, log_name)?;
    let tinderbox = obj_id(bot, TINDERBOX)?;
    let mut bank = nearest_bank(bot)?;
    bot.log(&format!("bank at ({},{})", bank.x, bank.z));
    let mut keep_log: Vec<u16> = vec![tinderbox, log];
    keep_log.extend(task_keep.iter().copied());
    bank_trip(bot, &bank, &keep_log, 0)?;
    // The tinderbox may sit in the bank (or was lost on a death): pull it
    // back, else block typed so the brain buys one (never a raw Sdk error
    // retried three times in six seconds).
    if pack_total(bot, tinderbox) == 0 {
        bank_withdraw_items(bot, &bank, &[tinderbox], 1)?;
        if pack_total(bot, tinderbox) == 0 {
            return Err(ScriptError::Blocked { missing: vec![TINDERBOX.to_string()] });
        }
    }
    fill_logs(bot, &bank, log)?;
    if pack_total(bot, log) == 0 {
        return Err(ScriptError::Blocked { missing: vec![log_name.to_string()] });
    }
    let outcome = run(bot, &spec, |step_bot, state| {
        check_events(step_bot)?;
        if pack_total(step_bot, log) == 0 {
            bank_trip(step_bot, &bank, &keep_log, 0)?;
            fill_logs(step_bot, &bank, log)?;
            if pack_total(step_bot, log) == 0 {
                return Err(ScriptError::Blocked { missing: vec![log_name.to_string()] });
            }
        }
        let mut lit = 0u32;
        while pack_total(step_bot, log) > 0 {
            match light_one(step_bot, log, tinderbox) {
                Err(error) if is_death_abort(&error) => {
                    step_bot.log("died firemaking, restocking to retry");
                    return Ok(StepControl::Continue);
                }
                Err(error) => return Err(error),
                Ok(()) => lit += 1,
            }
        }
        bank = nearest_bank(step_bot)?;
        bank_trip(step_bot, &bank, &task_keep, 0)?;
        step_bot.log(&format!("ok load {} lit {lit} {log_name} (firemaking={level})", state.units() + 1));
        Ok(StepControl::UnitDone)
    });
    outcome
}

/// Top the pack up with banked logs (nothing when the bank holds none or
/// the booth is far: the caller reports `blocked`, never an empty walk).
fn fill_logs(bot: &mut Bot, bank: &lifescape_gamedata::banks::Bank, log: u16) -> Result<(), ScriptError> {
    let free = free_slots(bot) as u32;
    if free == 0 {
        return Ok(());
    }
    if (bot.world.x - bank.x as i32).abs() + (bot.world.z - bank.z as i32).abs() > 12 {
        return Ok(());
    }
    bank_withdraw_items(bot, bank, &[log], free)?;
    Ok(())
}

/// Light one log where the bot stands, then step aside so the next fire
/// lands on a free tile (the line from catalogue §5.5, never a stack).
fn light_one(bot: &mut Bot, log: u16, tinderbox: u16) -> Result<(), ScriptError> {
    let had = pack_total(bot, log);
    let (com, slot) = inv_slot(bot, log)?;
    let (use_com, use_slot) = inv_slot(bot, tinderbox)?;
    bot.op_held_u(log, slot, com, tinderbox, use_slot, use_com)?;
    let here = (bot.world.x, bot.world.z);
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && pack_total(bot, log) >= had {
        bot.pump(5)?;
    }
    let _ = bot.walk_to(here.0 + 2, here.1, bot.world.level, 1500);
    if pack_total(bot, log) >= had {
        return Err(ScriptError::Failed(format!("log did not catch fire at {here:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{best_log, log_level, FIREMAKING_STAT, LOGS, TINDERBOX};
    use crate::kit::pack::obj_id;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn offline_bot() -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot
    }

    /// Catalogue §5.5 tiers: normal 1, oak 15, willow 30, yew 60.
    #[test]
    fn log_tiers_match_catalogue() {
        assert_eq!(LOGS.map(|(_, level)| level), [1, 15, 30, 60]);
        assert_eq!(log_level("logs"), Some(1));
        assert_eq!(log_level("WILLOW_LOGS"), Some(30), "case-insensitive");
        assert_eq!(log_level("magic_logs"), None, "members log is no F2P tier");
        assert_eq!(log_level("coins"), None, "junk never burns");
    }

    /// Tier pick: highest unlocked at the level, cheapest below.
    #[test]
    fn tier_pick_never_reaches_above() {
        assert_eq!(best_log(1), "logs");
        assert_eq!(best_log(29), "oak_logs");
        assert_eq!(best_log(30), "willow_logs");
        assert_eq!(best_log(99), "yew_logs");
    }

    /// Tool, logs and ashes all resolve through the live tables.
    #[test]
    fn tool_and_logs_resolve() {
        let bot = offline_bot();
        assert!(obj_id(&bot, TINDERBOX).is_ok(), "tinderbox resolves");
        assert!(obj_id(&bot, "ashes").is_ok(), "ashes resolve");
        for (name, _) in LOGS {
            assert!(obj_id(&bot, name).is_ok(), "{name} resolves");
        }
        assert_eq!(FIREMAKING_STAT, 11, "provenance firemaking id");
    }
}
