//! Full random-event handling: greet, survive, lamp, climb, maze legs
//! and the shared per-iteration check. Extends `randomevent` (which only
//! names zones and events) with the resolving behaviour from
//! `scripts/randomevents.py`. All bounds are in ticks, never sleep.

use lifescape_sdk::Bot;

use super::quest_common as kit;
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

pub fn climb_out(bot: &mut Bot) -> Result<bool, ScriptError> {
    let mut ridden: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..12 {
        if bot.world.level == 0 {
            bot.log(&format!("ok on ground at {:?}", bot.position()));
            return Ok(true);
        }
        // Known down-transitions nearest-first (exact loc and op each).
        // Bounded to the local area and to the closest few: a whole-world
        // scan with a walk attempt per row burns minutes for nothing.
        let mut downs: Vec<(i32, usize)> = Vec::new();
        for (index, row) in bot.transitions.iter().enumerate() {
            if row.disabled || row.from_tile.0 != bot.world.level || row.to_tile.0 >= row.from_tile.0 {
                continue;
            }
            if ridden.contains(&row.name) {
                continue;
            }
            let dist = (row.from_tile.1 - bot.world.x).abs() + (row.from_tile.2 - bot.world.z).abs();
            if dist > 30 {
                continue;
            }
            downs.push((dist, index));
        }
        downs.sort();
        downs.truncate(5);
        let mut rode = false;
        for (_, index) in downs {
            let row = bot.transitions[index].clone();
            if ride_transition(bot, &row)? {
                ridden.insert(row.name.clone());
                rode = true;
                break;
            }
        }
        if rode {
            continue;
        }
        if climb_single(bot)? {
            continue;
        }
        bot.log(&format!("nothing down from {:?}", bot.position()));
        return Ok(false);
    }
    Ok(bot.world.level == 0)
}

/// Ride one transition row: stand beside its tile, click its op, wait for
/// the level to drop. True when it moved us down. Every failure says why:
/// no silent fallback to a guessed ladder.
fn ride_transition(bot: &mut Bot, row: &lifescape_sdk::TransitionRow) -> Result<bool, ScriptError> {
    let (level, from_x, from_z) = row.from_tile;
    // Multi-tile stairs answer a click only on some of their tiles: try the
    // table's tile first, then the rest of the footprint.
    let mut candidates: Vec<(i32, i32)> = vec![(from_x, from_z)];
    for tile in &row.footprint {
        if *tile != (from_x, from_z) {
            candidates.push(*tile);
        }
    }
    for (click_x, click_z) in candidates {
        // The live map is the truth: the table's loc id and op are hints,
        // the tile may hold a twin (stairs vs stairs-top) with its own ops.
        let loc_id = bot
            .world
            .loc_at(click_x, click_z, level)
            .map(|(id, _, _)| id)
            .or_else(|| bot.world.static_locs.get(&(click_x, click_z, level)).map(|(id, _, _)| *id))
            .unwrap_or(row.loc);
        let op = downward_op(bot, loc_id).unwrap_or(row.op);
        // Dead socket, not a bad tile: typed instead of trying tiles.
        if let Err(error) = bot.approach(click_x, click_z, level, 1, 800) {
            if matches!(
                error,
                lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
            ) {
                return Err(error.into());
            }
            bot.log(&format!("transition {} tile ({click_x},{click_z}) unreachable: {error}", row.name));
            continue;
        }
        let before = bot.world.level;
        if let Err(error) = bot.op_loc(op, click_x, click_z, loc_id) {
            if matches!(
                error,
                lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
            ) {
                return Err(error.into());
            }
            bot.log(&format!("transition {} op {op} loc {loc_id} failed: {error}", row.name));
            continue;
        }
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target && bot.world.level == before {
            bot.pump(2)?;
        }
        if bot.world.level < before {
            bot.log(&format!("down {} to {:?}", row.name, bot.position()));
            return Ok(true);
        }
        bot.log(&format!("transition {} loc {loc_id} op {op} at ({click_x},{click_z}) left level {before}", row.name));
    }
    Ok(false)
}

/// 1-based op that goes down on this loc: "climb-down" preferred, then
/// plain "climb" (stairs toggle), else None when the dump says neither.
fn downward_op(bot: &Bot, loc_id: u16) -> Option<u8> {
    let def = bot.maps.as_ref()?.loc_def(loc_id as usize)?;
    let ops = lifescape_gamedata::loc_by_id(loc_id as usize)?.ops;
    let _ = def;
    let mut fallback = None;
    for (index, name) in ops.iter().enumerate() {
        let Some(label) = name.as_deref() else { continue };
        let lower = label.to_lowercase();
        if lower.contains("climb-down") || lower.contains("climb down") {
            return Some(index as u8 + 1);
        }
        if lower == "climb" && fallback.is_none() {
            fallback = Some(index as u8 + 1);
        }
    }
    fallback
}

fn climb_single(bot: &mut Bot) -> Result<bool, ScriptError> {
        let names: &[&str] = &["laddertop", "spiralstairstop", "stairstop", "ladder", "spiralstairs", "stairs"];
        let Ok((x, z, level, loc)) = kit::find_loc(bot, names, 150) else {
            bot.log(&format!("no ladder or stair near {:?}", bot.position()));
            return Ok(false);
        };
        bot.log(&format!("ladder at ({x},{z})"));
        // Dead socket, not a far ladder: typed instead of giving up.
        if let Err(error) = bot.approach(x, z, level, 3, 1500) {
            if matches!(
                error,
                lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
            ) {
                return Err(error.into());
            }
            return Ok(false);
        }
        let before = bot.world.level;
        bot.op_loc(1, x, z, loc)?;
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target && bot.world.level == before {
            bot.pump(5)?;
        }
        bot.log(&format!("now {:?}", bot.position()));
        Ok(bot.world.level != before)
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
