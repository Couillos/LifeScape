//! Full random-event handling: greet, survive, lamp, climb, maze legs
//! and the shared per-iteration check. Extends `randomevent` (which only
//! names zones and events) with the resolving behaviour.
//! All bounds are in ticks, never sleep.

use lifescape_sdk::Bot;

use crate::quests::common as kit;
use super::randomevent::{self};
use crate::{ScriptError, ScriptResult};

pub const EVENT_HOSTS: [&str; 3] = ["macro_mage", "macro_geni", "macro_dwarf"];
pub const HOSTILE_EVENTS: [&str; 2] = ["macro_swarm", "macro_triffidseed_angry"];

/// Talk to event hosts on sight. Their talk paths ask nothing; greeting
/// hands the reward instead of the fail-teleport.
pub fn greet(bot: &mut Bot, timeout: u64) -> Result<(), ScriptError> {
    for name in EVENT_HOSTS {
        let Some(slot) = bot.find_npc(name) else { continue };
        bot.log(&format!("greeting event host {name}"));
        // Orthogonal tiles only: the server refuses NPC clicks from a
        // diagonal, and approach stops at Manhattan distance. A dead
        // socket still aborts typed instead of trying hosts.
        if let Err(error) = bot.approach_npc(slot, timeout) {
            if matches!(
                error,
                lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
            ) {
                return Err(error.into());
            }
            bot.log(&format!("event host {name} unreachable"));
            continue;
        }
        let Some(slot) = bot.find_npc(name) else { continue };
        if let Err(error) = bot.op_npc(1, slot) {
            if matches!(
                error,
                lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
            ) {
                return Err(error.into());
            }
            continue;
        }
        let _ = kit::finish_dialog_like(bot, &[], timeout);
    }
    Ok(())
}

/// Talk to event hosts within 1 tile. No walking: this runs inside the
/// pump hook, and walking pumps, which would recurse into the hook.
pub fn greet_near(bot: &mut Bot, timeout: u64) -> Result<(), ScriptError> {
    for name in EVENT_HOSTS {
        let Some(slot) = bot.find_npc(name) else { continue };
        let Some(npc) = bot.world.npcs.get(&slot) else { continue };
        if (npc.x - bot.world.x).abs() + (npc.z - bot.world.z).abs() != 1 {
            continue;
        }
        bot.log(&format!("greeting nearby event host {name}"));
        let Some(slot) = bot.find_npc(name) else { continue };
        if bot.op_npc(1, slot).is_err() {
            continue;
        }
        let _ = kit::finish_dialog_like(bot, &[], timeout);
    }
    Ok(())
}

/// Pick a triffid plant's fruit when one is in click range: a ready seed
/// hands the fruit and clears the event, a growing one just complains.
pub fn pick_triffid(bot: &mut Bot) -> Result<(), ScriptError> {
    let mut seeds = bot.npcs_named(&["macro_triffidseed"]);
    if seeds.is_empty() {
        return Ok(());
    }
    seeds.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
    let (slot, seed_x, seed_z) = seeds[0];
    if (seed_x - bot.world.x).abs() + (seed_z - bot.world.z).abs() > 6 {
        return Ok(());
    }
    let _ = bot.op_npc(1, slot);
    Ok(())
}

/// Kill hostile event NPCs before they bleed the bot dry. True when none
/// is left (or none was there). Bounded in ticks.
pub fn survive(bot: &mut Bot, ticks: u64) -> Result<bool, ScriptError> {
    pick_triffid(bot)?;
    let deadline = bot.world.ticks + ticks;
    while bot.world.ticks < deadline {
        let mut foes = bot.npcs_named(&HOSTILE_EVENTS);
        if foes.is_empty() {
            return Ok(true);
        }
        foes.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
        let (slot, foe_x, foe_z) = foes[0];
        if (foe_x - bot.world.x).abs() + (foe_z - bot.world.z).abs() > 6 {
            return Ok(true);
        }
        let _ = bot.op_npc(2, slot);
        bot.pump(5)?;
    }
    Ok(bot.npcs_named(&HOSTILE_EVENTS).is_empty())
}

/// True when a genie lamp waits to be rubbed in the inventory.
pub fn lamp_in_pack(bot: &Bot) -> bool {
    lifescape_gamedata::objs_by_name("macro_genilamp").into_iter().next().is_some_and(|lamp| bot.inventory().get(&(lamp.obj_id as u16)).copied().unwrap_or(0) > 0)
}

/// Rub every genie lamp: runecraft when Rune Mysteries is done, else
/// prayer. One rub consumes one lamp; bounded so leftovers retry next
/// check(). True when a lamp was used.
pub fn rub_genie_lamp(bot: &mut Bot) -> Result<bool, ScriptError> {
    let lamp = kit::obj_id("macro_genilamp")?;
    if bot.inventory().get(&lamp).copied().unwrap_or(0) == 0 {
        return Ok(true);
    }
    let runemysteries = lifescape_gamedata::varps::id_by_name("runemysteries").unwrap_or(0) as u16;
    let (skill, skill_com) = if bot.world.varps.get(&runemysteries).copied().unwrap_or(0) >= 6 {
        ("runecraft", lifescape_gamedata::com_id("xplamp:runecraft"))
    } else {
        ("prayer", lifescape_gamedata::com_id("xplamp:prayer"))
    };
    let (Some(skill_com), Some(confirm_com)) = (skill_com, lifescape_gamedata::com_id("xplamp:confirm")) else {
        bot.log("genie lamp: xplamp buttons missing");
        return Ok(false);
    };
    let start = bot.inventory().get(&lamp).copied().unwrap_or(0);
    for _ in 0..5 {
        if bot.inventory().get(&lamp).copied().unwrap_or(0) == 0 {
            break;
        }
        let Ok((inv_com, slot)) = kit::inv_slot(bot, lamp) else { break };
        bot.op_held(1, lamp, slot, inv_com)?;
        let target = bot.world.ticks + 60;
        while bot.world.ticks < target {
            bot.pump(3)?;
            if bot.world.modal.as_ref().is_some_and(|modal| modal.layer == "main") {
                break;
            }
        }
        if bot.world.modal.is_none() {
            bot.log("genie lamp: picker did not open");
            break;
        }
        let seen = bot.world.messages.len();
        bot.if_button(skill_com)?;
        bot.pump(5)?;
        if bot.world.messages[seen..].iter().any(|line| {
            let lower = line.to_lowercase();
            lower.contains("rune mysteries") || lower.contains("druid") || lower.contains("members")
        }) {
            bot.log(&format!("genie lamp: {skill} refused"));
            bot.close_modal()?;
            bot.pump(3)?;
            break;
        }
        bot.if_button(confirm_com)?;
        let before = bot.inventory().get(&lamp).copied().unwrap_or(0);
        let target = bot.world.ticks + 60;
        while bot.world.ticks < target {
            bot.pump(3)?;
            if bot.inventory().get(&lamp).copied().unwrap_or(0) < before {
                break;
            }
            if bot.world.modal.is_some() {
                let _ = bot.chat_continue();
            }
        }
    }
    if bot.inventory().get(&lamp).copied().unwrap_or(0) < start {
        bot.log(&format!("genie lamp: wish granted to {skill}"));
        return Ok(true);
    }
    bot.log("genie lamp: still in inventory");
    Ok(false)
}

/// Climb back to the surface after a fail-teleport: ride the mapped
/// down-exits nearest-first via `travel_to` (same guards as before: 30
/// tiles, closest 5, 12 rounds). One bulk `transitions_list` fetch per
/// call; a dead socket or operator stop aborts typed, a refused ride
/// logs its typed cause and tries the next exit, and no mapped exit
/// nearby ends the rescue round (Ok(false)) instead of guessing a
/// ladder. A down service fails `nav-down` loud. A choice dialogue on
/// an exit surfaces as `Error::Travel(NeedsDialogueChoice)` in the log
/// line, never silent.
pub fn climb_out(bot: &mut Bot) -> Result<bool, ScriptError> {
    // Without the service the exits are unknown: typed `nav-down`, never
    // a guessed ladder.
    let rows = bot.transitions_list()?;
    for _ in 0..12 {
        if bot.world.level == 0 {
            bot.log(&format!("ok on ground at {:?}", bot.position()));
            return Ok(true);
        }
        // Known down-exits nearest-first (served rows, like before).
        // Bounded to the local area and to the closest few: planning one
        // ride per row is cheap, walking to every row in the world is not.
        let mut downs: Vec<(i32, _)> = Vec::new();
        for row in &rows {
            if row.disabled || row.from.level != bot.world.level || row.to.level >= row.from.level {
                continue;
            }
            let dist = (row.from.x - bot.world.x).abs() + (row.from.z - bot.world.z).abs();
            if dist > 30 {
                continue;
            }
            downs.push((dist, row.clone()));
        }
        downs.sort_by_key(|(dist, _)| *dist);
        downs.truncate(5);
        if downs.is_empty() {
            bot.log(&format!("nothing down from {:?}", bot.position()));
            return Ok(false);
        }
        let mut rode = false;
        for (_, row) in downs {
            match bot.travel_to(row.to) {
                Ok(()) => {
                    bot.log(&format!("down {} to {:?}", row.name, bot.position()));
                    rode = true;
                    break;
                }
                Err(
                    error @ (lifescape_sdk::Error::Interrupted(_)
                    | lifescape_sdk::Error::Io(_)
                    | lifescape_sdk::Error::Login(_)),
                ) => return Err(error.into()),
                Err(error) => bot.log(&format!("exit {} refused: {error}", row.name)),
            }
        }
        if !rode {
            bot.log(&format!("nothing down from {:?}", bot.position()));
            return Ok(false);
        }
    }
    Ok(bot.world.level == 0)
}

pub fn check(bot: &mut Bot) -> Result<bool, ScriptError> {
    greet(bot, 3000)?;
    survive(bot, 600)?;
    if lamp_in_pack(bot) && rub_genie_lamp(bot).is_err() {
        bot.log("genie lamp failed");
    }
    if randomevent::in_mime(bot) {
        bot.log(&randomevent::event_line(bot, randomevent::EventKind::Mime, false));
        return Ok(false);
    }
    if randomevent::cube_in_pack(bot) {
        bot.log(&randomevent::event_line(bot, randomevent::EventKind::Cube, false));
        return Ok(false);
    }
    if randomevent::in_maze(bot) {
        let solved = super::maze::solve(bot)?;
        bot.log(&randomevent::event_line(bot, randomevent::EventKind::Maze, solved));
        return Ok(solved);
    }
    if randomevent::in_fail_teleport(bot) {
        let escaped = bot.world.level == 0 || climb_out(bot)?;
        bot.log(&randomevent::event_line(bot, randomevent::EventKind::Tower, escaped));
        return Ok(escaped);
    }
    Ok(true)
}

/// Tick-bound wait for the bot to be free of events (fallback rescue
/// loop body). True when free on level 0 outside any event zone.
pub fn wait_clear(bot: &mut Bot, ticks: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + ticks;
    while bot.world.ticks < target {
        if check(bot)? {
            return Ok(true);
        }
        bot.pump(5)?;
    }
    Ok(false)
}

/// Greet hosts and clear what resolves inline; abort loudly when a
/// scripted rescue must take over. Thin wrapper for skill-bot loops.
pub fn run_check(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    if check(bot)? { Ok(0) } else { Ok(1) }
}
