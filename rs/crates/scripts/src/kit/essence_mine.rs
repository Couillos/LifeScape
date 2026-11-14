//! Rune essence mine geography and passage (split from
//! `activities/essence_run.rs`, file cap): the mine's map square, its
//! static and live locs, Aubury's teleport in and the exit portal out.

use lifescape_sdk::Bot;
use crate::activities::essence_run::{AUBURY, AUBURY_TELEPORT_OP, ESSENCE_WALK_TICKS, NEED_QUEST, PORTAL, PORTAL_STAND_TICKS};
use crate::kit::survival::is_walk_abort;
use crate::ScriptError;

/// Live essence rocks and portals around the bot (no service: the mine
/// streams its locs, static spawns cover the walk-in).
pub fn live_locs(bot: &Bot, name: &str, radius: i32) -> Vec<(i32, i32, u16)> {
    let mut found = Vec::new();
    for ((x, z, level), slot) in &bot.world.locs {
        let Some((loc, _, _)) = slot else { continue; };
        if *level == bot.world.level && (x - bot.world.x).abs() + (z - bot.world.z).abs() <= radius && (bot.tables.loc_name)(*loc).unwrap_or_default() == name {
            found.push((*x, *z, *loc));
        }
    }
    found.sort();
    found
}

/// Map square of the rune essence mine (`m45_75`: the four rocks and the
/// four exit portals, server `essence_mine_teleports` lands inside).
pub const MINE_SQUARE: (i32, i32) = (45, 75);

/// True inside the essence mine (level 0, map square 45_75).
pub fn in_mine(x: i32, z: i32, level: i32) -> bool {
    level == 0 && (x >> 6, z >> 6) == MINE_SQUARE
}

/// Essence rocks or exit portals around the bot by exact debugname: the
/// mine's static locs (one service batch: the server never streams a
/// static loc; live 2026-11-14, t48 stood in the mine at (2912,4825)
/// and the run read "Aubury did not teleport", no rock ever streamed)
/// plus any live one.
pub fn mine_locs(bot: &mut Bot, name: &str, radius: i32) -> Result<Vec<(i32, i32, u16)>, ScriptError> {
    let mut found = live_locs(bot, name, radius);
    for row in bot.static_locs_near(bot.world.x, bot.world.z, bot.world.level, radius)? {
        if (bot.tables.loc_name)(row.loc).unwrap_or_default() == name && !found.iter().any(|(x, z, _)| (*x, *z) == (row.x, row.z)) {
            found.push((row.x, row.z, row.loc));
        }
    }
    found.sort();
    Ok(found)
}

/// Aubury's teleport to the mine (op 4, no dialogue). Skipped when essence
/// rock already streams nearby. A quest refusal fails loud naming the quest.
pub fn teleport_to_mine(bot: &mut Bot) -> Result<(), ScriptError> {
    if in_mine(bot.world.x, bot.world.z, bot.world.level) {
        return Ok(());
    }
    // Out of view (live 2026-11-13: runs started in Lumbridge, Draynor or
    // Al Kharid failed "no Aubury near" at once), walk to his shop first.
    let slot = crate::kit::shop::npc_at_spawn(bot, "aubury", (AUBURY.0, AUBURY.1), ESSENCE_WALK_TICKS)?;
    bot.approach_npc(slot, ESSENCE_WALK_TICKS)?;
    bot.log("asking Aubury for the essence mine");
    bot.intend("teleporting to the rune essence mine");
    bot.op_npc(AUBURY_TELEPORT_OP, slot)?;
    let target = bot.world.ticks + 100;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if in_mine(bot.world.x, bot.world.z, bot.world.level) {
            bot.log("arrived in the essence mine");
            return Ok(());
        }
    }
    if crate::quests::common::has_message(bot, "rune mysteries") {
        bot.log(&format!("essence needs {NEED_QUEST} at {:?}", bot.position()));
        return Err(crate::kit::cycle::blocked_quest("runemysteries"));
    }
    Err(ScriptError::Failed(format!("Aubury did not teleport at {:?}", bot.position())))
}

/// Step back through the exit portal (op 1, `essence_mine.rs2` picks the
/// return tile from the entry side).
pub fn return_through_portal(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + 200;
    while bot.world.ticks < target {
        let (here_x, here_z) = (bot.world.x, bot.world.z);
        let nearest = mine_locs(bot, PORTAL, 60)?.into_iter().min_by_key(|(x, z, _)| (x - here_x).abs() + (z - here_z).abs());
        if let Some((x, z, loc)) = nearest {
            // The portal blocks its tile and answers only from an
            // orthogonal side (live 2026-11-14, t48: the planner's
            // diagonal fallback (2932,4816) read "I can't reach that!",
            // (2932,4815) stepped through to Aubury).
            for (stand_x, stand_z) in crate::devtools::sides_of(&[[i64::from(x), i64::from(z)]], bot.world.x, bot.world.z) {
                if let Err(error) = bot.walk_to(stand_x, stand_z, bot.world.level, PORTAL_STAND_TICKS) {
                    let error = ScriptError::from(error);
                    if is_walk_abort(&error) {
                        return Err(error);
                    }
                    continue;
                }
                if (bot.world.x, bot.world.z) != (stand_x, stand_z) {
                    continue;
                }
                bot.op_loc(1, x, z, loc)?;
                let back = bot.world.ticks + 100;
                while bot.world.ticks < back {
                    bot.pump(2)?;
                    if !in_mine(bot.world.x, bot.world.z, bot.world.level) {
                        bot.log("back from the essence mine");
                        return Ok(());
                    }
                }
            }
            return Err(ScriptError::Failed(format!("portal ({x},{z}) did not move us at {:?}", bot.position())));
        }
        bot.pump(5)?;
    }
    Err(ScriptError::Failed(format!("no essence portal near {:?}", bot.position())))
}

