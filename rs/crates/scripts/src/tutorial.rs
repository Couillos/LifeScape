//! Tutorial island driver: detect the stage from the server-shown
//! instruction box, then do what that stage asks. Works from any point
//! mid-quest.
//!
//! Stage signal: the instruction box (coms 6180-6184), never the tutorial
//! varp (the server does not transmit it) and never the inventory.

use lifescape_sdk::{Bot, Error};

use super::quest_common as kit;
use crate::{ScriptError, ScriptResult};

const COM_DESIGN_ACCEPT: u16 = 3651;
const COM_RUN: u16 = 153;
const COM_SMITH_COL1: u16 = 1119;
const SPELL_WIND_STRIKE: u16 = 1152;
const TUT_TAB: u8 = 3;
const TAB_WORN: u8 = 4;

fn obj(name: &str) -> Result<u16, ScriptError> {
    kit::obj_id(name)
}

fn check(pos: (i32, i32, i32), ok: bool, what: &str) -> Result<(), ScriptError> {
    println!("{} {what}", if ok { "ok" } else { "FAIL" });
    if ok {
        Ok(())
    } else {
        Err(ScriptError::Sdk(Error::State(format!("tutorial stuck at: {what} pos={pos:?}"))))
    }
}

fn has(bot: &Bot, name: &str, count: u32) -> bool {
    obj(name).ok().is_some_and(|id| bot.inventory().get(&id).copied().unwrap_or(0) >= count)
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

// (step key, instruction fragments) in tutorial order.
const STAGE_MATCHERS: &[(&str, &[&str])] = &[
    ("guide", &["getting started"]), ("controls_tab", &["spanner icon"]), ("door1", &["interacting with scenery"]),
    ("survival_start", &["moving around", "survival expert"]), ("open_inventory", &["flashing backpack icon"]),
    ("chop", &["cut down a tree"]), ("light_fire", &["building a fire"]), ("open_stats", &["bar graph icon"]),
    ("survival_food", &["these are your stats"]), ("catch", &["catch some shrimp", "fishing spot"]),
    ("cook_burnt", &["cooking your shrimp"]), ("cook_fresh", &["burning your shrimp"]),
    ("survival_gate", &["just cooked your first"]), ("chef_path", &["mini-map"]), ("chef", &["talk to the chef"]),
    ("make_dough", &["making dough"]), ("bake", &["cooking dough"]), ("music_tab", &["first loaf of bread"]),
    ("chef_door", &["the music player", "next door"]), ("run_controls", &["running man"]),
    ("run_button", &["click on the run button"]), ("quest_door", &["run to the next guide"]),
    ("quest_talk", &["talk with the quest guide"]), ("quest_tab", &["open the quest journal"]),
    ("quest_recap", &["your quest journal"]), ("ladder_down", &["enter some caves"]),
    ("mining_intro", &["mining and smithing"]), ("dig_tin_first", &["mine one tin ore"]),
    ("dig_copper", &["need some copper"]), ("dig_tin", &["need some tin"]), ("prospect", &["prospect"]),
    ("smelt", &["smelting"]), ("smith_talk", &["made a bronze bar"]), ("smith_anvil", &["smithing a dagger"]),
    ("mine_exit", &["finished this area"]), ("combat_intro", &["find out about combat"]),
    ("wield_tab", &["wielding weapons"]), ("wield", &["select wield from the menu"]),
    ("sword_talk", &["now holding your dagger"]), ("sword_equip", &["wield the sword and shield"]),
    ("combat_tab", &["crossed swords"]), ("rat_cage", &["tools needed for battle"]), ("melee", &["to attack the rat"]),
    ("melee_watch", &["sit back and watch"]), ("ranged_prep", &["made your first kill"]),
    ("ranged_kill", &["rat ranging"]), ("ladder_bank", &["completed the tasks here"]),
    ("bank_booth", &["bank of runescape"]), ("bank_door", &["this is your bank box"]),
    ("advisor_talk", &["financial advice"]), ("advisor_door", &["continue through the next door"]),
    ("chapel_path", &["follow the path to the chapel"]), ("prayer_tab", &["prayer menu", "flashing icon"]),
    ("chapel_recap", &["tell you about prayers"]), ("friends_tab", &["flashing smily"]),
    ("ignore_tab", &["this is your friends list"]), ("chapel_final", &["this is your ignore list"]),
    ("chapel_door", &["almost finished", "pass through the"]), ("terrova_talk", &["wizards house"]),
    ("magic_tab", &["open up your final menu"]), ("oops_runes", &["unhandled tutorial step"]),
    ("wind", &["several tries"]), ("wind_retry", &["didn't have any effect"]),
    ("final", &["almost completed the tutorial"]), ("wait", &["please wait"]),
];

fn detect_stage(bot: &Bot) -> Option<&'static str> {
    let text = instruction(bot).to_lowercase();
    if text.trim().is_empty() {
        return None;
    }
    STAGE_MATCHERS.iter().find(|(_, fragments)| fragments.iter().all(|fragment| text.contains(fragment))).map(|(key, _)| *key)
}

/// Talk + answer list with the tutorial's fail-fast stall detection.
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
                if bot.world.ticks - still_since.unwrap_or(0) > 75 {
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

// shortcut: the Rust SDK tracks no door state (no recent_doors/unseal),
// so a clicked door is re-walked raw instead of routed as known-open.
// Doors only answer orthogonal clicks (diagonals refuse), and the server
// lands the clicker one tile across: through means the far side, not one
// exact tile.
fn crossed_door(bot: &Bot, door: (i32, i32), start: (i32, i32)) -> bool {
    (bot.world.x - door.0) * (start.0 - door.0) + (bot.world.z - door.1) * (start.1 - door.1) < 0
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
            return Ok(());
        }
        let mut tiles = vec![(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)];
        tiles.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
        for (tile_x, tile_z) in tiles {
            if through(bot) {
                bot.log(&format!("ok {door_name}"));
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
                    return Ok(());
                }
            }
        }
    }
    check(bot.position(), through(bot), &format!("through {door_name}"))
}

/// Loc the server wants clicked: the hint arrow tile when set, else the
/// bot tile. The hint names the intended tree/rock/door; the nearest one to
/// the bot is not always it (fences sit between the survival trees).
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

fn op_loc_near(bot: &mut Bot, loc_names: &[&str], op: u8, done: impl Fn(&Bot) -> bool, timeout: u64) -> Result<bool, ScriptError> {
    let (x, z, level, loc) = find_target_loc(bot, loc_names, 150)?;
    let mut cands: Vec<(i32, i32)> =
        (-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (x + dx, z + dz))).filter(|tile| *tile != (x, z)).collect();
    cands.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
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
    let mut cands: Vec<(i32, i32)> =
        (-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (x + dx, z + dz))).filter(|tile| *tile != (x, z)).collect();
    cands.sort_by_key(|tile| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
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

fn design(bot: &mut Bot) -> Result<(), ScriptError> {
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && bot.world.modal.is_none() {
        bot.pump(5)?;
    }
    if bot.world.modal.is_none() {
        bot.log("ok no design screen, already validated");
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
    while bot.world.ticks < target {
        bot.pump(5)?;
        let Some(slot) = bot.find_npc("newbiefishing") else { continue };
        if bot.approach_npc(slot, 1500).is_err() {
            continue;
        }
        bot.op_npc(1, slot)?;
        let inner = bot.world.ticks + 50;
        while bot.world.ticks < inner {
            bot.pump(5)?;
            if bot.inventory().get(&raw).copied().unwrap_or(0) > had {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn cook_on_fire(bot: &mut Bot, raw_name: &str, want: &str) -> Result<(), ScriptError> {
    let raw = obj(raw_name)?;
    check(bot.position(), catch_shrimp(bot, 200)?, "shrimp caught for cooking")?;
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
    for _ in 0..4 {
        let mut slot = None;
        let target = bot.world.ticks + 50;
        while bot.world.ticks < target && slot.is_none() {
            bot.pump(5)?;
            slot = bot.find_npc("newbiegiantrat");
        }
        let Some(found) = slot else { continue };
        bot.op_npc(2, found)?;
        if wait_bones(bot, &bones, 150)? {
            bot.log("ok rat killed at range");
            return Ok(());
        }
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
        "catch" => check(bot.position(), catch_shrimp(bot, 200)?, "shrimp caught"),
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
        "ladder_down" => {
            if let Ok((x, z, _level, loc)) = kit::find_loc(bot, &["newbieladdertop1"], 150) {
                stand_orthogonal(bot, x, z)?;
                bot.op_loc(1, x, z, loc)?;
            }
            check(bot.position(), wait_moved(bot, 20)?, "down the mine ladder")
        }
        "mining_intro" => talk_step(bot, "mining_instructor", (3079, 9505), &["Nope, I'm ready to move on!"], "miner"),
        "prospect" => prospect(bot),
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
            if !kit::talk(bot, "banker", None, 150)? {
                let (x, z, _level, loc) = kit::find_loc(bot, &["newbiebankbooth"], 150)?;
                bot.approach(x, z, bot.world.level, 2, 1500)?;
                bot.op_loc(1, x, z, loc)?;
                bot.pump(10)?;
            }
            if bot.world.modal.is_some() {
                finish_dialog(bot, &["Yes."], 300)?;
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
            let start = (bot.world.x, bot.world.z, bot.world.level);
            talk_step(bot, "magic_instructor", (3141, 3089), &["Yes."], "terrova final")?;
            let target = bot.world.ticks + 100;
            while bot.world.ticks < target {
                bot.pump(5)?;
                if (bot.world.x, bot.world.z, bot.world.level) != start {
                    bot.log("ok teleported to mainland");
                    return Ok(());
                }
            }
            check(bot.position(), false, "teleported to mainland")
        }
        _ => Err(ScriptError::Usage(format!("tutorial stuck: unknown stage {stage:?}"))),
    }
}

/// Click a tutorial wall gate from on top of its tile: gates that face away
/// refuse every adjacent stand under the server wall reach rule, but their
/// own tile is standable and clicks from on top always reach. Steps are
/// direct (server-validated); the router refuses gate tiles. The first
/// click opens the gate for good (halves deleted), so a missing gate means
/// walk straight through.
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

/// Step onto an exact tile, verifying arrival: the planner reports Ok
/// without moving when its mesh refuses the tile.
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
        if bot.world.ticks - started > 20000 {
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
                // Empty box right after a step or a relogin: re-click the
                // tab to resend the current stage, several rounds. Tab
                // clicks only ever advance to the next required click-tab
                // stage, so this cannot skip real work.
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
            bot.log(&format!("stage {stage} hint={:?} flash={:?}", bot.world.hint, bot.world.side_flash));
        }
        step(bot, stage)?;
        bot.pump(5)?;
    }
}
