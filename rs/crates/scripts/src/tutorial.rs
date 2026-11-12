//! Tutorial island driver: stage from the server instruction box
//! (coms 6180-6184; the tutorial varp is transmit=false so it is never
//! used), with a geography-plus-kit fallback when the box is empty or
//! delayed. Resumable from any point mid-quest.

use lifescape_sdk::{Bot, Error};

use crate::quests::common as kit;
use crate::tutorial_detect::{all_matches, fallback_stage, has, obj};
use crate::{ScriptError, ScriptResult};

const COM_DESIGN_ACCEPT: u16 = 3651;
const COM_RUN: u16 = 153;
const COM_SMITH_COL1: u16 = 1119;
const SPELL_WIND_STRIKE: u16 = 1152;
const TUT_TAB: u8 = 3;
const TAB_WORN: u8 = 4;

fn check(pos: (i32, i32, i32), ok: bool, what: &str) -> Result<(), ScriptError> {
    println!("{} {what}", if ok { "ok" } else { "FAIL" });
    if ok {
        Ok(())
    } else {
        Err(ScriptError::Sdk(Error::State(format!("tutorial stuck at: {what} pos={pos:?}"))))
    }
}

/// Ticks since `started`; survives tick resets (t02: 1439 -> 0).
fn elapsed_ticks(now: u64, started: u64) -> u64 {
    now.saturating_sub(started)
}

fn wait_inv(bot: &mut Bot, name: &str, count: u32, timeout: u64) -> Result<bool, ScriptError> {
    let id = obj(name)?;
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if bot.inventory().get(&id).copied().unwrap_or(0) >= count {
            return Ok(true);
        }
    }
    Ok(false)
}

fn wait_moved(bot: &mut Bot, timeout: u64) -> Result<bool, ScriptError> {
    let before = (bot.world.x, bot.world.z, bot.world.level);
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if (bot.world.x, bot.world.z, bot.world.level) != before {
            bot.pump(10)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn instruction(bot: &Bot) -> String {
    let mut texts = bot.chat_texts();
    texts.sort();
    let title = texts.iter().find(|(com, _)| *com == 6180).map(|(_, text)| text.clone()).unwrap_or_default();
    let body: Vec<String> = texts.into_iter().filter(|(com, _)| (6181..=6184).contains(com)).map(|(_, text)| text).collect();
    format!("{title}\n{}", body.join(" "))
}

// (step key, instruction fragments) live in tutorial_detect::STAGE_MATCHERS,
// audited against the exact server box corpus.

fn detect_stage(bot: &Bot) -> Option<&'static str> {
    let text = instruction(bot).to_lowercase();
    if text.trim().is_empty() {
        return None;
    }
    let hits = all_matches(&text);
    if hits.len() > 1 {
        let snippet: String = text.chars().take(160).collect();
        bot.log(&format!("ambiguous tutorial box {snippet:?} candidates {hits:?}"));
    }
    hits.into_iter().next()
}

/// Talk + answers with fail-fast stall detection.
fn finish_dialog(bot: &mut Bot, answers: &[&str], timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    let mut answered: Vec<String> = Vec::new();
    let mut still_since: Option<u64> = None;
    while bot.world.ticks < target {
        let before: Vec<String> = bot.chat_texts().into_iter().map(|(_, text)| text).collect();
        let had_modal = bot.world.modal.is_some();
        bot.pump(5)?;
        let pending: Vec<String> =
            bot.chat_options().into_iter().map(|(_, text)| text).filter(|text| answers.contains(&text.as_str()) && !answered.contains(text)).collect();
        if !pending.is_empty() {
            let picked = answers.iter().find(|answer| pending.iter().any(|text| text == *answer)).copied().unwrap_or(pending[0].as_str());
            bot.chat_choose(picked)?;
            answered.push(picked.to_string());
            still_since = None;
        } else if bot.world.modal.is_some() {
            bot.chat_continue()?;
            let now: Vec<String> = bot.chat_texts().into_iter().map(|(_, text)| text).collect();
            if now == before && had_modal {
                if still_since.is_none() {
                    still_since = Some(bot.world.ticks);
                }
                if elapsed_ticks(bot.world.ticks, still_since.unwrap_or(0)) > 75 {
                    return Err(ScriptError::Sdk(Error::Timeout(format!("tutorial stuck: dialog not closing at {:?} modal={:?}", bot.position(), bot.world.modal))));
                }
            } else {
                still_since = None;
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

// No door state in the SDK: re-walk raw, orthogonal clicks only (the
// server lands the clicker one tile across).
fn crossed_door(bot: &Bot, door: (i32, i32), start: (i32, i32)) -> bool {
    (bot.world.x - door.0) * (start.0 - door.0) + (bot.world.z - door.1) * (start.1 - door.1) < 0
}

/// Finish standing on the far tile: the crossing click lands the bot
/// anywhere past the door (advisor door: 3131 vs beyond 3132) and a fresh
/// refusal with no motion after kills the run on the rejection watch.
/// The breather first lets the watch arm on the pre-step tile, so the
/// step itself resets it; best effort, the crossing is already proven.
fn settle_beyond(bot: &mut Bot, beyond: (i32, i32)) -> Result<(), ScriptError> {
    if (bot.world.x, bot.world.z) == beyond {
        return Ok(());
    }
    bot.pump(10)?;
    if (bot.world.x, bot.world.z) == beyond {
        return Ok(());
    }
    let _ = bot.walk_to(beyond.0, beyond.1, bot.world.level, 1500);
    Ok(())
}

fn pass_door(bot: &mut Bot, door_name: &str, beyond: (i32, i32)) -> Result<(), ScriptError> {
    let (x, z, _level, loc) = kit::find_loc(bot, &[door_name], 150)?;
    let start = (bot.world.x, bot.world.z);
    let through = |bot: &Bot| (bot.world.x, bot.world.z) == beyond || crossed_door(bot, (x, z), start);
    // The server only walks through from the door's own axis (a wrong-side
    // click parks the bot on the door tile): rotate all four orthogonal
    // stands instead of hammering the nearest one.
    for _ in 0..2 {
        if through(bot) {
            bot.log(&format!("ok {door_name}"));
            settle_beyond(bot, beyond)?;
            return Ok(());
        }
        let mut tiles = vec![(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)];
        tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if through(bot) {
                bot.log(&format!("ok {door_name}"));
                settle_beyond(bot, beyond)?;
                return Ok(());
            }
            if !step_onto(bot, tile_x, tile_z, 1000) {
                continue;
            }
            bot.op_loc(1, x, z, loc)?;
            let target = bot.world.ticks + 15;
            while bot.world.ticks < target {
                bot.pump(1)?;
                if through(bot) {
                    bot.log(&format!("ok {door_name}"));
                    settle_beyond(bot, beyond)?;
                    return Ok(());
                }
            }
        }
    }
    check(bot.position(), through(bot), &format!("through {door_name}"))
}

/// Mine region (z ~9500) vs surface island (z ~3100), same level 0: a
/// walk_to across regions is unroutable, never attempt it.
fn is_mine_region(z: i32) -> bool {
    z > 9000
}

/// Down the mine ladder (surface -> underground). The post-kill box can
/// outlive our position (death respawn, mid-quest resume): ranged_prep
/// climbs back down instead of walking across map regions.
fn climb_down_mine(bot: &mut Bot) -> Result<(), ScriptError> {
    if let Ok((x, z, _level, loc)) = kit::find_loc(bot, &["newbieladdertop1"], 150) {
        stand_orthogonal(bot, x, z)?;
        bot.op_loc(1, x, z, loc)?;
    }
    check(bot.position(), wait_moved(bot, 20)?, "down the mine ladder")?;
    check(bot.position(), is_mine_region(bot.world.z), "back underground for ranged prep")
}

/// Hint tile when set, else the bot tile (nearest is not always the target).
fn target_ref(bot: &Bot) -> (i32, i32) {
    match bot.world.hint {
        Some(lifescape_sdk::world::Hint::Coord(x, z, _)) => (i32::from(x), i32::from(z)),
        _ => (bot.world.x, bot.world.z),
    }
}

fn find_target_loc(bot: &mut Bot, loc_names: &[&str], radius: i32) -> Result<(i32, i32, i32, u16), ScriptError> {
    let from = target_ref(bot);
    match kit::find_loc_from(bot, loc_names, radius, from) {
        Ok(found) => Ok(found),
        Err(_) if from != (bot.world.x, bot.world.z) => kit::find_loc(bot, loc_names, radius),
        Err(failed) => Err(failed),
    }
}

/// Stand tiles, orthogonal first (diagonals never answer).
fn ordered_stands(x: i32, z: i32, bot_x: i32, bot_z: i32) -> Vec<(i32, i32)> {
    let mut cands: Vec<(i32, i32)> = (-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (x + dx, z + dz))).filter(|tile| *tile != (x, z)).collect();
    cands.sort_by_key(|tile| {
        let orthogonal = (tile.0 == x) != (tile.1 == z);
        let dist = (tile.0 - bot_x).abs() + (tile.1 - bot_z).abs();
        ((!orthogonal) as u8, dist)
    });
    cands
}

fn op_loc_near(bot: &mut Bot, loc_names: &[&str], op: u8, done: impl Fn(&Bot) -> bool, timeout: u64) -> Result<bool, ScriptError> {
    let (x, z, level, loc) = find_target_loc(bot, loc_names, 150)?;
    let cands = ordered_stands(x, z, bot.world.x, bot.world.z);
    let target = bot.world.ticks + timeout;
    for (cx, cz) in cands {
        if bot.world.ticks > target {
            break;
        }
        if bot.walk_to(cx, cz, level, 1250).is_err() {
            continue;
        }
        let seen = bot.world.messages.len();
        bot.op_loc(op, x, z, loc)?;
        let inner = bot.world.ticks + 20;
        while bot.world.ticks < inner {
            bot.pump(2)?;
            if done(bot) {
                return Ok(true);
            }
            if bot.world.messages[seen..].iter().any(|line| line.to_lowercase().contains("can't reach that")) {
                break;
            }
        }
    }
    Ok(false)
}

fn use_held_on_loc_near(bot: &mut Bot, loc_names: &[&str], tool: u16, done: impl Fn(&Bot) -> bool, timeout: u64) -> Result<bool, ScriptError> {
    let (x, z, level, loc) = find_target_loc(bot, loc_names, 150)?;
    let cands = ordered_stands(x, z, bot.world.x, bot.world.z);
    let target = bot.world.ticks + timeout;
    for (cx, cz) in cands {
        if bot.world.ticks > target {
            break;
        }
        if bot.walk_to(cx, cz, level, 1250).is_err() {
            continue;
        }
        let (use_com, use_slot) = kit::inv_slot(bot, tool)?;
        bot.op_loc_u(x, z, loc, tool, u16::from(use_slot), use_com)?;
        let inner = bot.world.ticks + 20;
        while bot.world.ticks < inner {
            bot.pump(2)?;
            if done(bot) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn talk_step(bot: &mut Bot, npc: &str, near: (i32, i32), answers: &[&str], what: &str) -> Result<(), ScriptError> {
    check(bot.position(), kit::talk(bot, npc, Some(near), 150)?, &format!("{what} found"))?;
    finish_dialog(bot, answers, 300)?;
    Ok(())
}

// -- north: guide, survival, chef -------------------------------------------
fn guide(bot: &mut Bot) -> Result<(), ScriptError> {
    talk_step(bot, "basics_instructor", (3095, 3107), &["No, thank you."], "guide")
}

fn is_chat_modal(modal: &Option<lifescape_sdk::world::Modal>) -> bool {
    modal.as_ref().is_some_and(|open| open.layer == "chat")
}

fn design(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && bot.world.modal.is_none() {
        bot.pump(5)?;
    }
    if bot.world.modal.is_none() {
        bot.log("ok no design screen, already validated");
        return Ok(());
    }
    if is_chat_modal(&bot.world.modal) {
        // Resume with a leftover dialog (interrupted run, post-relogin):
        // the accept button lives on the `player_kit` main interface
        // (server `[if_button,player_kit:accept]`), never in a chat box —
        // firing it there answers "I can't reach that!" and the rejection
        // watch kills the run. Close it, the stage loop re-talks.
        bot.log("ok leftover dialog closed, stage loop re-drives");
        bot.close_modal()?;
        return Ok(());
    }
    bot.if_button(COM_DESIGN_ACCEPT)?;
    check(bot.position(), wait_modal_closed(bot, 50)?, "design accepted")
}

fn wait_modal_closed(bot: &mut Bot, timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if bot.world.modal.is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn chop(bot: &mut Bot) -> Result<(), ScriptError> {
    let axe = obj("bronze_axe")?;
    if !bot.inventory().contains_key(&axe) {
        // Defaut 1 offline cause: the box already asks for a tree while
        // the axe never landed (login lag, or a mid-quest resume that
        // skipped the grant). Re-talk before failing loud (t03/t05).
        let _ = talk_step(bot, "survival", (3103, 3095), &[], "survival re-grant");
        let _ = wait_inv(bot, "bronze_axe", 1, 60);
    }
    check(bot.position(), bot.inventory().contains_key(&axe), "axe held")?;
    let logs = obj("newbielogs")?;
    while bot.inventory().get(&logs).copied().unwrap_or(0) < 2 {
        if bot.world.modal.is_some() {
            bot.chat_continue()?;
            bot.pump(5)?;
        }
        let had = bot.inventory().get(&logs).copied().unwrap_or(0);
        check(bot.position(), op_loc_near(bot, &["newbietree", "newbietree2", "newbiedeadtree2"], 1, |candidate| candidate.inventory().get(&logs).copied().unwrap_or(0) > had, 120)?, "logs chopped")?;
    }
    Ok(())
}

fn light_fire(bot: &mut Bot) -> Result<(), ScriptError> {
    let logs = obj("newbielogs")?;
    let tinderbox = obj("tinderbox")?;
    if bot.inventory().get(&logs).copied().unwrap_or(0) < 1 {
        chop(bot)?;
    }
    for _ in 0..3 {
        if bot.inventory().get(&logs).copied().unwrap_or(0) < 1 {
            chop(bot)?;
        }
        let here = (bot.world.x, bot.world.z);
        let (com, slot) = kit::inv_slot(bot, logs)?;
        let (use_com, use_slot) = kit::inv_slot(bot, tinderbox)?;
        bot.op_held_u(logs, slot, com, tinderbox, use_slot, use_com)?;
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target {
            bot.pump(5)?;
            if let Ok((fire_x, fire_z, _, _)) = kit::find_loc(bot, &["fire"], 150) {
                if (fire_x - here.0).abs() + (fire_z - here.1).abs() <= 1 {
                    bot.log("ok fire lit");
                    return Ok(());
                }
            }
        }
        let _ = bot.walk_to(here.0 + 2, here.1, bot.world.level, 1500);
    }
    check(bot.position(), false, "fire lighting")
}

fn catch_shrimp(bot: &mut Bot, timeout: u64) -> Result<bool, ScriptError> {
    let raw = obj("newbieraw_shrimp")?;
    let had = bot.inventory().get(&raw).copied().unwrap_or(0);
    let target = bot.world.ticks + timeout;
    // One spot can go stale (ghost slot answers nothing): rotate through
    // all visible spots instead of hammering the nearest forever.
    let mut dud: Vec<u16> = Vec::new();
    while bot.world.ticks < target {
        bot.pump(5)?;
        let mut spots: Vec<u16> = bot.find_npcs("newbiefishing").into_iter().filter(|slot| !dud.contains(slot)).collect();
        if spots.is_empty() {
            dud.clear();
            spots = bot.find_npcs("newbiefishing");
        }
        let Some(slot) = spots.into_iter().next() else { continue };
        let _ = bot.approach_npc(slot, 30);
        bot.op_npc(1, slot)?;
        let inner = bot.world.ticks + 60;
        while bot.world.ticks < inner {
            bot.pump(5)?;
            if bot.inventory().get(&raw).copied().unwrap_or(0) > had {
                return Ok(true);
            }
        }
        dud.push(slot);
    }
    Ok(false)
}

fn cook_on_fire(bot: &mut Bot, raw_name: &str, want: &str) -> Result<(), ScriptError> {
    let raw = obj(raw_name)?;
    if !has(bot, raw_name, 1) {
        check(bot.position(), catch_shrimp(bot, 300)?, "shrimp caught for cooking")?;
    }
    check(bot.position(), wait_inv(bot, raw_name, 1, 90)?, &format!("has {raw_name}"))?;
    // Tutorial fires burn out after 100-200 ticks: short cook rounds with a
    // relight between them, never one long hammering of a dead fire.
    for _ in 0..3 {
        if kit::find_loc(bot, &["fire"], 15).is_err() {
            bot.log("no fire nearby, lighting");
            light_fire(bot)?;
        }
        let want_id = obj(want)?;
        if let Ok((fire_x, fire_z, _, _)) = kit::find_loc(bot, &["fire"], 150) {
            bot.log(&format!("cooking on fire at {fire_x},{fire_z}"));
        }
        if use_held_on_loc_near(bot, &["fire"], raw, |candidate| candidate.inventory().contains_key(&want_id), 100)? {
            return Ok(());
        }
        bot.log(&format!("{want} missed this fire, relighting"));
    }
    check(bot.position(), false, &format!("{want} cooked"))
}

fn chef(bot: &mut Bot) -> Result<(), ScriptError> {
    talk_step(bot, "cook_instructor", (3080, 3084), &["Nothing, thanks."], "chef")?;
    let flour = obj("newbie_pot_flour")?;
    check(bot.position(), wait_inv(bot, "newbie_pot_flour", 1, 60)?, "flour+water given").map(|_| { let _ = flour; })
}

fn make_dough(bot: &mut Bot) -> Result<(), ScriptError> {
    if !has(bot, "newbie_pot_flour", 1) || !has(bot, "bucket_water", 1) {
        chef(bot)?;
    }
    check(bot.position(), wait_inv(bot, "newbie_pot_flour", 1, 60)?, "flour+water ready")?;
    check(bot.position(), wait_inv(bot, "bucket_water", 1, 60)?, "flour+water ready")?;
    let water = obj("bucket_water")?;
    let flour = obj("newbie_pot_flour")?;
    let (com, slot) = kit::inv_slot(bot, water)?;
    let (use_com, use_slot) = kit::inv_slot(bot, flour)?;
    bot.op_held_u(water, slot, com, flour, use_slot, use_com)?;
    check(bot.position(), wait_inv(bot, "bread_dough", 1, 90)?, "dough mixed")
}

fn bake_bread(bot: &mut Bot) -> Result<(), ScriptError> {
    if !has(bot, "bread_dough", 1) {
        make_dough(bot)?;
    }
    let dough = obj("bread_dough")?;
    let bread = obj("bread")?;
    check(bot.position(), use_held_on_loc_near(bot, &["newbierange"], dough, |candidate| candidate.inventory().contains_key(&bread), 400)?, "bread baked")
}

fn click_tab(bot: &mut Bot, tab: u8, what: &str) -> Result<(), ScriptError> {
    bot.tut_clickside(tab)?;
    bot.pump(10)?;
    bot.log(&format!("ok {what} opened"));
    Ok(())
}

// -- mine ------------------------------------------------------------------
fn prospect(bot: &mut Bot) -> Result<(), ScriptError> {
    check(bot.position(), op_loc_near(bot, &["newbiecopperrock"], 2, |candidate| kit::has_message(candidate, "contains copper"), 400)?, "copper prospected")?;
    check(bot.position(), op_loc_near(bot, &["newbietinrock"], 2, |candidate| kit::has_message(candidate, "contains tin"), 400)?, "tin prospected")?;
    talk_step(bot, "mining_instructor", (3079, 9505), &["Nope, I'm ready to move on!"], "miner again")?;
    check(bot.position(), wait_inv(bot, "bronze_pickaxe", 1, 60)?, "pickaxe given")
}

fn dig(bot: &mut Bot, rock: &str, want: &str) -> Result<(), ScriptError> {
    let ore = obj(want)?;
    let had = bot.inventory().get(&ore).copied().unwrap_or(0);
    check(bot.position(), op_loc_near(bot, &[rock], 1, |candidate| candidate.inventory().get(&ore).copied().unwrap_or(0) > had, 120)?, &format!("{want} mined"))
}

fn smelt(bot: &mut Bot) -> Result<(), ScriptError> {
    if !has(bot, "copper_ore", 1) {
        dig(bot, "newbiecopperrock", "copper_ore")?;
    }
    if !has(bot, "tin_ore", 1) {
        dig(bot, "newbietinrock", "tin_ore")?;
    }
    let copper = obj("copper_ore")?;
    let bar = obj("bronze_bar")?;
    check(bot.position(), use_held_on_loc_near(bot, &["newbiefurnace"], copper, |candidate| candidate.inventory().contains_key(&bar), 400)?, "bar smelted")
}

fn smith_anvil(bot: &mut Bot) -> Result<(), ScriptError> {
    if !has(bot, "bronze_bar", 1) {
        smelt(bot)?;
    }
    let bar = obj("bronze_bar")?;
    let dagger = obj("bronze_dagger")?;
    check(bot.position(), use_held_on_loc_near(bot, &["anvil"], bar, |candidate| candidate.world.modal.is_some(), 400)?, "smithing interface")?;
    bot.inv_button(1, dagger, 0, COM_SMITH_COL1)?;
    bot.close_modal()?;
    bot.pump(5)?;
    check(bot.position(), wait_inv(bot, "bronze_dagger", 1, 90)?, "dagger smithed")
}

// -- combat ----------------------------------------------------------------
fn equip_held(bot: &mut Bot, name: &str) -> Result<(), ScriptError> {
    let id = obj(name)?;
    check(bot.position(), wait_inv(bot, name, 1, 60)?, &format!("{name} held"))?;
    let (com, slot) = kit::inv_slot(bot, id)?;
    bot.op_held(2, id, slot, com)?;
    bot.pump(10)?;
    Ok(())
}

fn melee(bot: &mut Bot) -> Result<(), ScriptError> {
    let bones = obj("newbiebones")?;
    let deadline = bot.world.ticks + 400;
    let mut target: Option<u16> = None;
    let mut tries = 0u32;
    while bot.world.ticks < deadline {
        bot.pump(5)?;
        // Bones drop on the kill and despawn fast: stop fighting as soon as
        // they are visible instead of waiting out the target slot (which a
        // respawned rat can reuse, hiding the kill). Only once attacking,
        // so stale bones from an old kill cannot fake it.
        if target.is_some() && !bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, 15, Some(bones)).is_empty() {
            break;
        }
        if let Some(slot) = target {
            if bot.world.npcs.contains_key(&slot) {
                continue;
            }
            if wait_bones(bot, &bones, 200)? {
                break;
            }
            target = None;
            continue;
        }
        let Some(slot) = bot.find_npc("newbiegiantrat") else { continue };
        tries += 1;
        if tries % 10 == 1 {
            bot.log(&format!("melee try {tries} at {:?} rat={slot}", bot.position()));
        }
        let seen = bot.world.messages.len();
        if let Some(npc) = bot.world.npcs.get(&slot) {
            let _ = bot.approach(npc.x, npc.z, bot.world.level, 2, 1500);
        }
        bot.op_npc(2, slot)?;
        target = Some(slot);
        if bot.world.messages[seen..].iter().any(|line| line.to_lowercase().contains("can't reach that")) {
            target = None;
        }
    }
    check(bot.position(), wait_bones(bot, &bones, 100)?, "rat killed in melee")?;
    let _ = bot.take(None, Some("newbiebones"), 3000);
    bot.log("ok bones picked up");
    Ok(())
}

fn wait_bones(bot: &mut Bot, bones: &u16, timeout: u64) -> Result<bool, ScriptError> {
    let target = bot.world.ticks + timeout;
    while bot.world.ticks < target {
        bot.pump(5)?;
        if !bot.world.ground_items(bot.world.x, bot.world.z, bot.world.level, 15, Some(*bones)).is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ranged_kill(bot: &mut Bot) -> Result<(), ScriptError> {
    let bones = obj("newbiebones")?;
    // Rotate spots: one ghost slot must not eat all four shots.
    let mut dud: Vec<u16> = Vec::new();
    for _ in 0..4 {
        let mut spots: Vec<u16> = bot.find_npcs("newbiegiantrat").into_iter().filter(|slot| !dud.contains(slot)).collect();
        if spots.is_empty() {
            dud.clear();
            spots = bot.find_npcs("newbiegiantrat");
        }
        let Some(found) = spots.into_iter().next() else { continue };
        bot.op_npc(2, found)?;
        if wait_bones(bot, &bones, 150)? {
            bot.log("ok rat killed at range");
            return Ok(());
        }
        dud.push(found);
    }
    check(bot.position(), false, "ranged rat kill")
}

// -- south -----------------------------------------------------------------
fn wind_strike(bot: &mut Bot) -> Result<(), ScriptError> {
    let air = obj("airrune")?;
    let mind = obj("mindrune")?;
    let _ = bot.walk_to(3141, 3089, bot.world.level, 3000);
    let target = bot.world.ticks + 250;
    while bot.world.ticks < target {
        if bot.inventory().get(&air).copied().unwrap_or(0) < 1 || bot.inventory().get(&mind).copied().unwrap_or(0) < 1 {
            terrova_runes(bot)?;
            continue;
        }
        let mut slot = None;
        let inner = bot.world.ticks + 50;
        while bot.world.ticks < inner && slot.is_none() {
            bot.pump(5)?;
            slot = bot.find_npc("newbiechicken");
        }
        let Some(found) = slot else { continue };
        if let Some(npc) = bot.world.npcs.get(&found) {
            let _ = bot.approach(npc.x, npc.z, bot.world.level, 5, 1500);
        }
        bot.op_npc_t(found, SPELL_WIND_STRIKE)?;
        let inner = bot.world.ticks + 20;
        while bot.world.ticks < inner {
            bot.pump(2)?;
            if !bot.world.npcs.contains_key(&found) {
                bot.log("ok chicken killed with magic");
                return Ok(());
            }
        }
    }
    Ok(())
}

fn terrova_runes(bot: &mut Bot) -> Result<(), ScriptError> {
    if has(bot, "airrune", 1) && has(bot, "mindrune", 1) {
        bot.log("ok runes already held");
        return Ok(());
    }
    talk_step(bot, "magic_instructor", (3141, 3089), &["Yes."], "terrova runes")?;
    check(bot.position(), wait_inv(bot, "airrune", 1, 60)?, "runes held")
}

// -- driver ------------------------------------------------------------------
fn step(bot: &mut Bot, stage: &str) -> Result<(), ScriptError> {
    match stage {
        "guide" => guide(bot),
        "controls_tab" => click_tab(bot, 12, "controls tab"),
        "door1" => pass_door(bot, "newbie_door1", (3100, 3107)),
        "survival_start" => talk_step(bot, "survival", (3103, 3095), &[], "survival"),
        "open_inventory" => {
            click_tab(bot, TUT_TAB, "inventory tab")?;
            check(bot.position(), wait_inv(bot, "bronze_axe", 1, 30)?, "axe+tinderbox given")
        }
        "chop" => chop(bot),
        "light_fire" => light_fire(bot).map(|_| ()),
        "open_stats" => click_tab(bot, 1, "stats tab"),
        "survival_food" => {
            talk_step(bot, "survival", (3103, 3095), &[], "survival again")?;
            check(bot.position(), wait_inv(bot, "net", 1, 60)?, "net given")
        }
        "catch" => check(bot.position(), catch_shrimp(bot, 300)?, "shrimp caught"),
        "cook_burnt" => cook_on_fire(bot, "newbieraw_shrimp", "burntfish1"),
        "cook_fresh" => cook_on_fire(bot, "newbieraw_shrimp", "shrimp"),
        "survival_gate" => {
            let target = bot.world.ticks + 500;
            while bot.world.x >= 3089 && bot.world.ticks < target {
                if (bot.world.x, bot.world.z) != (3089, 3091) && (bot.world.x, bot.world.z) != (3089, 3092) {
                    if let Ok((x, z, _level, loc)) = kit::find_loc(bot, &["newbiegateclosed"], 150) {
                        let _ = bot.approach(x, z, bot.world.level, 1, 1500);
                        bot.op_loc(1, x, z, loc)?;
                        bot.pump(35)?;
                    }
                }
                let _ = bot.walk_to(3085, 3092, 0, 400);
            }
            check(bot.position(), bot.world.x < 3089, "through survival gate")?;
            bot.walk_to(3080, 3086, 0, 6000)?;
            Ok(())
        }
        "chef_path" | "chef" => chef(bot),
        "make_dough" => make_dough(bot),
        "bake" => bake_bread(bot),
        "music_tab" => click_tab(bot, 13, "music tab"),
        "chef_recap" => talk_step(bot, "cook_instructor", (3080, 3084), &["Nothing, thanks."], "chef recap"),
        "chef_door" => pass_door(bot, "newbie_door3", (3070, 3090)),
        "run_controls" | "run_button" => {
            click_tab(bot, 12, "controls tab")?;
            bot.if_button(COM_RUN)?;
            bot.pump(15)?;
            bot.log("ok run toggled (verified at quest door)");
            Ok(())
        }
        "quest_door" => pass_door(bot, "newbie_door4", (3086, 3128)),
        "quest_talk" | "quest_recap" => talk_step(bot, "quest_instructor", (3086, 3121), &["Nope, I'm ready to move on!"], "quest guide"),
        "quest_tab" => click_tab(bot, 2, "quest tab"),
        "ladder_down" => climb_down_mine(bot),
        "mining_intro" => talk_step(bot, "mining_instructor", (3079, 9505), &["Nope, I'm ready to move on!"], "miner"),
        "prospect" | "prospect_done" => prospect(bot),
        "dig_tin_first" => dig(bot, "newbietinrock", "tin_ore"),
        "dig_copper" => dig(bot, "newbiecopperrock", "copper_ore"),
        "dig_tin" => dig(bot, "newbietinrock", "tin_ore"),
        "smelt" => smelt(bot),
        "smith_talk" => {
            talk_step(bot, "mining_instructor", (3079, 9505), &["Nope, I'm ready to move on!"], "miner for hammer")?;
            check(bot.position(), wait_inv(bot, "hammer", 1, 60)?, "hammer given")
        }
        "smith_anvil" => smith_anvil(bot),
        "mine_exit" => cross_gate(bot, &["newbiedoor4"], (3095, 9502), "out of the mine"),
        "combat_intro" => talk_step(bot, "combat_instructor", (3107, 9510), &["Nope, I'm ready to move on!"], "combat instructor"),
        "wield_tab" => click_tab(bot, TAB_WORN, "worn tab"),
        "wield" => {
            click_tab(bot, TAB_WORN, "worn tab")?;
            let dagger = obj("bronze_dagger")?;
            if kit::inv_slot(bot, dagger).is_err() {
                bot.log("ok dagger already wielded");
                return Ok(());
            }
            equip_held(bot, "bronze_dagger")
        }
        "sword_talk" => {
            talk_step(bot, "combat_instructor", (3107, 9510), &["Nope, I'm ready to move on!"], "combat instructor again")?;
            check(bot.position(), wait_inv(bot, "bronze_sword", 1, 60)?, "sword+shield given")
        }
        "sword_equip" => {
            equip_held(bot, "bronze_sword")?;
            equip_held(bot, "wooden_shield")
        }
        "combat_tab" => click_tab(bot, 0, "combat tab"),
        "rat_cage" => {
            // The pit interior is fixed geography: near (3109,9519) means
            // inside. Never short-circuit on mere proximity from outside:
            // the gate still needs a click to advance the stage.
            for _ in 0..3 {
                if (bot.world.x - 3109).abs() + (bot.world.z - 9519).abs() <= 2 {
                    bot.log("ok inside rat pit");
                    return Ok(());
                }
                cross_gate(bot, &["newbiedoor5"], (3110, 9518), "inside rat pit")?;
                bot.pump(5)?;
            }
            check(bot.position(), (bot.world.x - 3109).abs() + (bot.world.z - 9519).abs() <= 2, "inside rat pit")
        }
        "melee" | "melee_watch" => melee(bot),
        "ranged_prep" => {
            // Post-kill box with a surface position (death respawn,
            // mid-quest resume): Vannaka is underground, the cross-region
            // walk_to is unroutable and scan_and_talk would swallow it.
            if !is_mine_region(bot.world.z) {
                bot.log("ranged prep on surface, climbing back down");
                climb_down_mine(bot)?;
            }
            // Exit east through the rat gate: click from the inside stand
            // (the gate faces the pit), the handler parks us on the gate,
            // then walk out. The gate stays open after the first entry.
            if let Ok((x, z, _level, loc)) = kit::find_loc(bot, &["newbiedoor5"], 150) {
                if step_onto(bot, x - 1, z, 1500) {
                    let _ = bot.op_loc(1, x, z, loc);
                    bot.pump(10)?;
                }
                if (bot.world.x, bot.world.z) == (x, z) && bot.walk_to(x + 1, z, bot.world.level, 3000).is_err() {
                    let _ = bot.step_to(x + 1, z, 30);
                }
            }
            talk_step(bot, "combat_instructor", (3107, 9510), &["Nope, I'm ready to move on!"], "combat instructor ranged")?;
            check(bot.position(), wait_inv(bot, "shortbow", 1, 60)?, "bow given")?;
            equip_held(bot, "shortbow")?;
            equip_held(bot, "bronze_arrow")
        }
        "ranged_kill" => ranged_kill(bot),
        "ladder_bank" => {
            if let Ok((x, z, _level, loc)) = kit::find_loc(bot, &["newbieladder2"], 150) {
                stand_orthogonal(bot, x, z)?;
                bot.op_loc(1, x, z, loc)?;
            }
            check(bot.position(), wait_moved(bot, 20)?, "up the bank ladder")
        }
        "bank_booth" => {
            // The banker offers Yes/No (standard multi2 header); Yes opens
            // the bank, a main-side interface. Page to the choice, answer
            // once, then close. A stuck dialog (open but textless) gets
            // closed and retried.
            for _ in 0..2 {
                if !kit::talk(bot, "banker", None, 150)? {
                    let (x, z, _level, loc) = kit::find_loc(bot, &["newbiebankbooth"], 150)?;
                    bot.approach(x, z, bot.world.level, 2, 1500)?;
                    bot.op_loc(1, x, z, loc)?;
                    bot.pump(10)?;
                }
                let target = bot.world.ticks + 90;
                while bot.world.ticks < target && bot.world.modal.is_some() && bot.chat_options().is_empty() {
                    bot.chat_continue()?;
                    bot.pump(5)?;
                }
                if !bot.chat_options().is_empty() {
                    break;
                }
                bot.close_modal()?;
                bot.pump(5)?;
            }
            if !bot.chat_options().is_empty() {
                bot.chat_choose("Yes.")?;
                bot.pump(10)?;
            }
            bot.close_modal()?;
            bot.pump(5)?;
            check(bot.position(), wait_modal_closed(bot, 60)?, "bank visited")
        }
        "bank_door" => pass_door(bot, "newbie_door6", (3127, 3124)),
        "advisor_talk" => talk_step(bot, "account_instructor", (3127, 3124), &["No thanks."], "advisor"),
        "advisor_door" => pass_door(bot, "newbie_door7", (3132, 3124)),
        "chapel_path" | "chapel_recap" | "chapel_final" => talk_step(bot, "brother_noob", (3125, 3106), &["Nope, I'm ready to move on!"], "brother"),
        "prayer_tab" => click_tab(bot, 5, "prayer tab"),
        "friends_tab" => click_tab(bot, 8, "friends tab"),
        "ignore_tab" => click_tab(bot, 8, "ignore tab"),
        "chapel_door" => pass_door(bot, "newbie_door8", (3121, 3098)),
        "terrova_talk" => talk_step(bot, "magic_instructor", (3141, 3089), &["Yes."], "terrova"),
        "magic_tab" => {
            click_tab(bot, 6, "magic tab")?;
            terrova_runes(bot)
        }
        "oops_runes" => terrova_runes(bot),
        "wind" | "wind_retry" => {
            wind_strike(bot)?;
            Ok(())
        }
        "final" => {
            if bot.world.x > 3200 {
                bot.log("ok already on the mainland");
                return Ok(());
            }
            talk_step(bot, "magic_instructor", (3141, 3089), &["Yes."], "terrova final")?;
            // Only the mainland teleport counts: the approach walk inside
            // talk_step moves the bot too, which faked success here before.
            let target = bot.world.ticks + 100;
            while bot.world.ticks < target {
                bot.pump(5)?;
                if bot.world.x > 3200 {
                    bot.log("ok teleported to mainland");
                    return Ok(());
                }
            }
            check(bot.position(), false, "teleported to mainland")
        }
        _ => Err(ScriptError::Failed(format!("tutorial stuck: unknown stage {stage:?}"))),
    }
}

/// Wall gate from on top of its tile (orthogonal stands only); an opened
/// gate is gone, so a missing gate means walk through.
fn cross_gate(bot: &mut Bot, loc_names: &[&str], beyond: (i32, i32), what: &str) -> Result<(), ScriptError> {
    let start = (bot.world.x, bot.world.z);
    let mut last_gate: Option<(i32, i32)> = None;
    for _ in 0..3 {
        if (bot.world.x, bot.world.z) == beyond {
            bot.log(&format!("ok {what}"));
            return Ok(());
        }
        let Ok((x, z, level, loc)) = kit::find_loc(bot, loc_names, 150) else {
            let _ = bot.walk_to(beyond.0, beyond.1, bot.world.level, 3000);
            bot.pump(5)?;
            continue;
        };
        last_gate = Some((x, z));
        if (bot.world.x, bot.world.z) == beyond || crossed_door(bot, (x, z), start) {
            bot.log(&format!("ok {what}"));
            return Ok(());
        }
        let _ = bot.approach(x, z, level, 1, 1500);
        for _ in 0..3 {
            if (bot.world.x, bot.world.z) == (x, z) {
                break;
            }
            let (dx, dz) = (x - bot.world.x, z - bot.world.z);
            let (next_x, next_z) = if dx.abs() >= dz.abs() {
                (bot.world.x + dx.signum(), bot.world.z)
            } else {
                (bot.world.x, bot.world.z + dz.signum())
            };
            if bot.step_to(next_x, next_z, 15).is_err() {
                break;
            }
            bot.pump(2)?;
        }
        if (bot.world.x, bot.world.z) != (x, z) {
            bot.pump(5)?;
            continue;
        }
        bot.op_loc(1, x, z, loc)?;
        let target = bot.world.ticks + 15;
        while bot.world.ticks < target {
            bot.pump(1)?;
            if (bot.world.x, bot.world.z) == beyond || crossed_door(bot, (x, z), start) {
                bot.log(&format!("ok {what}"));
                return Ok(());
            }
        }
    }
    let done = (bot.world.x, bot.world.z) == beyond
        || last_gate.is_some_and(|gate| crossed_door(bot, gate, start));
    check(bot.position(), done, &format!("through {what}"))
}

/// Exact tile step: the planner reports Ok without moving on refusal.
fn step_onto(bot: &mut Bot, tile_x: i32, tile_z: i32, timeout: u64) -> bool {
    if (bot.world.x, bot.world.z) == (tile_x, tile_z) {
        return true;
    }
    if bot.walk_to(tile_x, tile_z, bot.world.level, timeout).is_err() {
        return false;
    }
    (bot.world.x, bot.world.z) == (tile_x, tile_z)
}

fn stand_orthogonal(bot: &mut Bot, x: i32, z: i32) -> Result<(), ScriptError> {
    let mut tiles = vec![(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)];
    tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for (tile_x, tile_z) in tiles {
        if step_onto(bot, tile_x, tile_z, 1500) {
            return Ok(());
        }
    }
    Err(ScriptError::Sdk(Error::Unreachable(format!("no orthogonal stand near {x},{z} at {:?}", bot.position()))))
}

/// Run the tutorial island from any stage to the mainland teleport.
pub fn run_tutorial(bot: &mut Bot, _args: &[String]) -> ScriptResult {
    bot.log(&format!("start {:?}", bot.position()));
    design(bot)?;
    bot.close_modal()?;
    bot.pump(5)?;
    // Tutorial texts lag login by several ticks: wait for the pack too.
    let mut target = bot.world.ticks + 50;
    while bot.world.ticks < target && bot.inventory().is_empty() {
        bot.pump(5)?;
    }
    target = bot.world.ticks + 50;
    while bot.world.ticks < target && instruction(bot).trim().is_empty() {
        bot.pump(5)?;
    }
    let started = bot.world.ticks;
    let mut last_key = String::new();
    let mut repeats = 0u32;
    loop {
        if bot.world.x > 3200 && bot.world.level == 0 {
            bot.log(&format!("done {:?}", bot.position()));
            return Ok(0);
        }
        if elapsed_ticks(bot.world.ticks, started) > 20000 {
            return Err(ScriptError::Sdk(Error::Timeout(format!("tutorial stuck: 20000 ticks elapsed at {:?}", bot.position()))));
        }
        let Some(stage) = detect_stage(bot) else {
            bot.pump(10)?;
            if let Some(retry) = detect_stage(bot) {
                bot.intend(&format!("tutorial: {retry}"));
                step(bot, retry)?;
                bot.pump(5)?;
                continue;
            }
            bot.log(&format!("no stage match for instruction: {:?}", &instruction(bot)[..instruction(bot).len().min(200)]));
            if instruction(bot).trim().is_empty() {
                // Empty box after a step or relogin: re-click the tab,
                // several rounds (tab clicks cannot skip real work).
                let mut recovered = false;
                for _ in 0..5 {
                    bot.tut_clickside(TUT_TAB)?;
                    bot.pump(15)?;
                    if detect_stage(bot).is_some() {
                        recovered = true;
                        break;
                    }
                }
                if !recovered {
                    // Silent advance (completed -> open_player_controls
                    // sends no text): the run button advances it here.
                    bot.if_button(COM_RUN)?;
                    bot.pump(15)?;
                    recovered = detect_stage(bot).is_some();
                }
                if !recovered {
                    // Empty box after login, death or resume: re-enter from
                    // geography plus kit instead of failing the run.
                    if let Some(stage) = fallback_stage(bot) {
                        bot.log(&format!("empty box at {:?}, fallback: {stage}", bot.position()));
                        bot.intend(&format!("tutorial: {stage}"));
                        step(bot, stage)?;
                        bot.pump(5)?;
                        continue;
                    }
                    return Err(ScriptError::Sdk(Error::Timeout(format!("tutorial stuck: no instruction at {:?}", bot.position()))));
                }
                continue;
            }
            return Err(ScriptError::Sdk(Error::State(format!("tutorial stuck: unmapped instruction at {:?}", bot.position()))));
        };
        bot.intend(&format!("tutorial: {stage}"));
        if stage == "wait" {
            bot.pump(5)?;
            continue;
        }
        if stage == last_key {
            repeats += 1;
            if repeats >= 5 {
                return Err(ScriptError::Sdk(Error::Timeout(format!("tutorial stuck: stage {stage} not progressing at {:?}", bot.position()))));
            }
        } else {
            last_key = stage.to_string();
            repeats = 0;
            let boxed: String = instruction(bot).chars().take(160).collect();
            bot.log(&format!("stage {stage} hint={:?} flash={:?} box={boxed:?}", bot.world.hint, bot.world.side_flash));
        }
        step(bot, stage)?;
        bot.pump(5)?;
    }
}

#[cfg(test)]
mod tests {
    use super::{elapsed_ticks, is_chat_modal, is_mine_region};
    use crate::tutorial_detect::STAGE_MATCHERS;
    use lifescape_sdk::world::Modal;

    /// t02: ticks reset 1439 -> 0 on the mine transition; no panic.
    #[test]
    fn elapsed_survives_tick_reset() {
        assert_eq!(elapsed_ticks(100, 50), 50);
        assert_eq!(elapsed_ticks(5, 1439), 0, "reset: no panic, no false timeout");
    }

    /// Defaut 2 (btjk4cpk, mine, dialogue mineur resté ouvert) : le préambule
    /// tirait l'accept du design dans la boîte de chat — "I can't reach
    /// that!" frais + 20 ticks sans bouger (FAIL verify), ou 50 ticks
    /// d'attente modale (FAIL design accepted). Seul le chat ferme.
    #[test]
    fn leftover_chat_modal_never_takes_design_accept() {
        let chat = Some(Modal { layer: "chat".to_string(), interface_id: 4887 });
        assert!(is_chat_modal(&chat), "resume leftover closes, never presses accept");
        let main = Some(Modal { layer: "main".to_string(), interface_id: 1 });
        assert!(!is_chat_modal(&main), "design screen keeps its accept press");
        assert!(!is_chat_modal(&None), "no modal stays a clean start");
    }

    /// Defaut 3 (Iron, boucle ranged_prep depuis la surface 3072,3133) :
    /// le texte "made your first kill" est authentique (varp post-kill,
    /// pas un stale) mais Vannaka vit au sous-sol (z ~9500) : le walk_to
    /// inter-regions est imroutable, scan_and_talk l'avale, talk FAIL
    /// "not found" 150 ticks plus tard. Le garde remonte par l'echelle.
    #[test]
    fn mine_region_splits_surface_from_mine() {
        assert!(is_mine_region(9510), "pit Vannaka 3107,9510");
        assert!(!is_mine_region(3133), "surface Iron 3072,3133");
    }

    /// Q1 : le seul texte qui matche ranged_prep (serveur
    /// tut_chatbox_steps.rs2 tutorial_step_first_kill_melee).
    #[test]
    fn ranged_prep_matcher_is_post_kill_box() {
        let prep = STAGE_MATCHERS.iter().find(|(key, _)| *key == "ranged_prep").expect("ranged_prep row");
        assert!(prep.1.iter().any(|fragment| fragment.contains("made your first kill")), "varp post-kill, got {:?}", prep.1);
    }
    #[test]
    fn chop_matcher_names_the_tree_step() {
        // Defaut 1 offline pin (replay-nav style, no live run): the t03/t05
        // `FAIL axe held` shows the box already asks for a tree, so the
        // matcher must name `chop` and `chop` must re-grant before failing.
        let keys: Vec<&str> = STAGE_MATCHERS.iter().map(|(key, _)| *key).collect();
        assert!(keys.contains(&"chop"), "tree step pinned, got {keys:?}");
        let chop = STAGE_MATCHERS.iter().find(|(key, _)| *key == "chop").expect("chop row");
        assert!(chop.1.iter().any(|fragment| fragment.contains("cut down a tree")), "t03/t05 text, got {:?}", chop.1);
        assert!(keys.contains(&"open_inventory"), "grant step precedes chop");
    }
}
