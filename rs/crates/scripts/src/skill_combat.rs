//! Melee farmer: best F2P spot for the level, best banked gear, banked food.
//! Rotates attack/strength/defence, eats when hurt, loots every drop except
//! bones, banks when the pack fills (gather loop, bank trip, sell, upgrade).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::waits::wait_ticks;
use lifescape_sdk::Bot;

use crate::skill_common::{bury_bones, check_events, free_slots, hitpoints, obj_id, walk_checked};
use crate::{ScriptError, ScriptResult};

const EAT_FRACTION: f64 = 0.55;
const FOOD_TARGET: usize = 10;
const LOOT_RADIUS: i32 = 6;
const STYLE_KILLS: u32 = 3;
const MELEE_ORDER: [&str; 3] = ["attack", "strength", "defence"];
const ARMOUR_SLOTS: [i64; 9] = [0, 1, 2, 4, 5, 7, 9, 10, 12];
/// Server wearpos slots: 3 is the weapon, 5 the shield.
const WEAPON_SLOT: i64 = 3;
const WORN_COM_NAME: &str = "wornitems:wear";

/// Combat style buttons per weapon category (player_attackstyles data).
fn style_buttons(category: &str, skill: &str) -> Option<&'static str> {
    match (category, skill) {
        ("weapon_slash", "attack") => Some("combat_hacksword:hack0"),
        ("weapon_slash", "strength") => Some("combat_hacksword:hack1"),
        ("weapon_slash", "defence") => Some("combat_hacksword:hack3"),
        ("weapon_stab", "attack") => Some("combat_stabsword:stab0"),
        ("weapon_stab", "strength") => Some("combat_stabsword:stab1"),
        ("weapon_stab", "defence") => Some("combat_stabsword:stab3"),
        ("weapon_blunt", "attack") => Some("combat_blunt:blunt0"),
        ("weapon_blunt", "strength") => Some("combat_blunt:blunt1"),
        ("weapon_blunt", "defence") => Some("combat_blunt:blunt2"),
        ("weapon_axe", "attack") => Some("combat_axe:axe0"),
        ("weapon_axe", "strength") => Some("combat_axe:axe1"),
        ("weapon_axe", "defence") => Some("combat_axe:axe3"),
        ("weapon_2h_sword", "attack") => Some("combat_heavysword:heavy0"),
        ("weapon_2h_sword", "strength") => Some("combat_heavysword:heavy1"),
        ("weapon_2h_sword", "defence") => Some("combat_heavysword:heavy3"),
        ("weapon_pickaxe", "attack") => Some("combat_pickaxe:pickaxe0"),
        ("weapon_pickaxe", "strength") => Some("combat_pickaxe:pickaxe1"),
        ("weapon_pickaxe", "defence") => Some("combat_pickaxe:pickaxe3"),
        ("weapon_spear", "attack" | "strength") => Some("combat_spear:spear0"),
        ("weapon_spear", "defence") => Some("combat_spear:spear3"),
        ("unarmed", "attack") => Some("combat_unarmed:unarmed0"),
        ("unarmed", "strength") => Some("combat_unarmed:unarmed1"),
        ("unarmed", "defence") => Some("combat_unarmed:unarmed2"),
        _ => None,
    }
}

/// A scimitar's speed beats a 2h sword's raw bonus: one-handed slashers win
/// ties (strategy, not game data).
fn weapon_weight(category: &str) -> f64 {
    match category {
        "weapon_slash" => 1.0,
        "weapon_stab" => 0.9,
        "weapon_axe" | "weapon_blunt" => 0.85,
        "weapon_pickaxe" => 0.7,
        "weapon_2h_sword" | "weapon_spear" => 0.6,
        _ => 0.5,
    }
}

fn gear_score(entry: &knowledge::equipment::Wearable, slot: i64) -> f64 {
    if slot == WEAPON_SLOT {
        let attack = entry.attack.iter().map(|(_, bonus)| *bonus).max().unwrap_or(0) as f64;
        let category = entry.category.as_deref().unwrap_or("");
        return (attack + 2.0 * entry.strength as f64) * weapon_weight(category);
    }
    entry.defence.iter().map(|(_, bonus)| *bonus).sum::<i64>() as f64 + 2.0 * entry.strength as f64
        + entry.prayer as f64
}

fn levels_by_name(bot: &Bot) -> Vec<(String, i64)> {
    knowledge::equipment::STATS
        .iter()
        .map(|(name, index)| {
            let level = bot.world.stats.get(&(*index as u8)).map(|(level, _)| i64::from(*level)).unwrap_or(1);
            (name.to_string(), level)
        })
        .collect()
}

fn melee_level(bot: &Bot) -> i64 {
    let at = |index: u8| bot.world.stats.get(&index).map(|(level, _)| i64::from(*level)).unwrap_or(1);
    (at(0) + at(2) + at(1)) / 3
}

/// Display names of worn + banked + carried gear, with their obj ids.
fn available_gear(bot: &Bot) -> HashMap<u16, String> {
    let mut found = HashMap::new();
    for id in bot.inventory().keys() {
        if let Some(name) = (bot.tables.obj_name)(*id) {
            found.insert(*id, name);
        }
    }
    if let Some(worn_com) = (bot.tables.com_id)(WORN_COM_NAME) {
        if let Some(slots) = bot.world.inv.get(&worn_com) {
            for (_, (held, _)) in slots {
                if let Some(name) = (bot.tables.obj_name)(*held) {
                    found.insert(*held, name);
                }
            }
        }
    }
    for (id, _) in bot.bank_items() {
        if let Some(name) = (bot.tables.obj_name)(id) {
            found.insert(id, name);
        }
    }
    found
}

/// Best wearable per slot among the available obj ids.
fn gear_plan(bot: &Bot, available: &HashMap<u16, String>) -> HashMap<i64, Vec<knowledge::equipment::Wearable>> {
    let levels = levels_by_name(bot);
    let refs: Vec<(&str, i64)> = levels.iter().map(|(name, level)| (name.as_str(), *level)).collect();
    let ids: std::collections::HashSet<u16> = available.keys().copied().collect();
    let mut plan: HashMap<i64, Vec<knowledge::equipment::Wearable>> = HashMap::new();
    for entry in knowledge::equipment::all_wearables() {
        let Ok(obj) = obj_id(bot, &entry.name) else {
            continue;
        };
        if !ids.contains(&obj) || !knowledge::equipment::requirements_met(&entry, &refs) {
            continue;
        }
        for slot in [WEAPON_SLOT].into_iter().chain(ARMOUR_SLOTS) {
            if knowledge::equipment::covers_slot(&entry, slot) {
                plan.entry(slot).or_default().push(entry.clone());
            }
        }
    }
    for (slot, cands) in plan.iter_mut() {
        cands.sort_by(|a, b| gear_score(b, *slot).partial_cmp(&gear_score(a, *slot)).unwrap_or(std::cmp::Ordering::Equal));
        if *slot == WEAPON_SLOT {
            // Prefer a one-handed weapon so the shield slot stays useful.
            cands.sort_by_key(|entry| knowledge::equipment::covers_slot(entry, 5));
        }
    }
    plan
}

fn worn_obj(bot: &Bot, slot: i64) -> Option<u16> {
    let worn_com = (bot.tables.com_id)(WORN_COM_NAME)?;
    bot.world.inv.get(&worn_com)?.get(&(slot as u8)).map(|(held, _)| *held)
}

/// Wear an entry from the pack. True when it ends up worn.
fn equip(bot: &mut Bot, entry: &knowledge::equipment::Wearable) -> Result<bool, ScriptError> {
    let obj = obj_id(bot, &entry.name)?;
    if worn_obj(bot, entry.wearpos) == Some(obj) {
        return Ok(true);
    }
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    let slot = bot.backpack().1.into_iter().find(|(_, (held, _))| *held == obj).map(|(slot, _)| slot);
    let Some(slot) = slot else {
        return Ok(false);
    };
    let seen = bot.world.messages.len();
    bot.op_held(2, obj, slot, pack_com)?;
    let target = bot.world.ticks + 25;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if worn_obj(bot, entry.wearpos) == Some(obj) {
            return Ok(true);
        }
        if bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|line| {
            let lowered = line.to_lowercase();
            lowered.contains("not a high enough level") || lowered.contains("free space")
        }) {
            return Ok(false);
        }
    }
    Ok(false)
}

fn equip_plan(bot: &mut Bot, plan: &HashMap<i64, Vec<knowledge::equipment::Wearable>>) -> Result<HashMap<i64, String>, ScriptError> {
    let mut equipped: HashMap<i64, String> = HashMap::new();
    for slot in [WEAPON_SLOT].into_iter().chain(ARMOUR_SLOTS) {
        if slot == 5 && equipped.contains_key(&WEAPON_SLOT) {
            let weapon = equipped.get(&WEAPON_SLOT).cloned().unwrap_or_default();
            if knowledge::equipment::by_name(&weapon).is_some_and(|entry| knowledge::equipment::covers_slot(&entry, 5)) {
                continue;
            }
        }
        let cands = plan.get(&slot).cloned().unwrap_or_default();
        for entry in cands {
            if equip(bot, &entry)? {
                equipped.insert(slot, entry.name.clone());
                break;
            }
        }
    }
    Ok(equipped)
}

fn current_weapon_category(bot: &Bot) -> String {
    match worn_obj(bot, WEAPON_SLOT) {
        Some(obj) => knowledge::equipment::all_wearables()
            .into_iter()
            .find(|entry| obj_id(bot, &entry.name).is_ok_and(|id| id == obj))
            .and_then(|entry| entry.category)
            .unwrap_or_else(|| "unarmed".to_string()),
        None => "unarmed".to_string(),
    }
}

fn set_style(bot: &mut Bot, skill: &str) -> Result<(), ScriptError> {
    let category = current_weapon_category(bot);
    if let Some(dotted) = style_buttons(&category, skill) {
        if let Some(com) = (bot.tables.com_id)(dotted) {
            bot.if_button(com)?;
            bot.pump(3)?;
        }
    }
    Ok(())
}

/// Eat the first edible thing in the pack.
fn eat(bot: &mut Bot) -> Result<bool, ScriptError> {
    let found = bot.backpack().1.into_iter().find(|(_, (held, _))| {
        (bot.tables.obj_name)(*held).is_some_and(|name| knowledge::food::heal(&name) > 0)
    });
    let Some((slot, (held, _))) = found else {
        return Ok(false);
    };
    let Some(pack_com) = bot.backpack().0 else {
        return Ok(false);
    };
    bot.op_held(1, held, slot, pack_com)?;
    bot.pump(3)?;
    Ok(true)
}

fn food_count(bot: &Bot) -> u32 {
    bot.backpack()
        .1
        .into_iter()
        .filter(|(_, (held, _))| (bot.tables.obj_name)(*held).is_some_and(|name| knowledge::food::heal(&name) > 0))
        .map(|(_, (_, count))| count)
        .sum()
}

/// Withdraw the best banked food up to the target. Returns food carried.
fn withdraw_food(bot: &mut Bot) -> Result<u32, ScriptError> {
    let banked: Vec<String> =
        bot.bank_items().keys().filter_map(|id| (bot.tables.obj_name)(*id)).collect();
    let refs: Vec<&str> = banked.iter().map(String::as_str).collect();
    let Some((chosen, _)) = knowledge::food::best_food(&refs) else {
        return Ok(food_count(bot));
    };
    let food_id = obj_id(bot, &chosen)?;
    let need = (FOOD_TARGET as u32).saturating_sub(food_count(bot));
    if need > 0 {
        let _ = bot.bank_withdraw(food_id, need, 50);
    }
    Ok(food_count(bot))
}

fn nearest_bank_tile(bot: &Bot) -> Option<(i32, i32, i32)> {
    knowledge::banks::all_banks()
        .into_iter()
        .filter(|bank| !bank.members && bank.level == i64::from(bot.world.level))
        .min_by_key(|bank| (bank.x - i64::from(bot.world.x)).abs() + (bank.z - i64::from(bot.world.z)).abs())
        .map(|bank| (bank.level as i32, bank.x as i32, bank.z as i32))
}

/// Deposit loot, withdraw the best gear and food for the current levels.
fn prepare_at_bank(bot: &mut Bot) -> Result<bool, ScriptError> {
    let Some((level, bank_x, bank_z)) = nearest_bank_tile(bot) else {
        let available = available_gear(bot);
        let plan = gear_plan(bot, &available);
        equip_plan(bot, &plan)?;
        return Ok(false);
    };
    bot.intend(&format!("banking at ({bank_x},{bank_z})"));
    // Dead socket or operator stop, not a missed bank: typed to the
    // session instead of a quiet equip-and-continue.
    match walk_checked(bot, bank_x, bank_z, 15000) {
        Ok(true) => {}
        Ok(false) => {
            let available = available_gear(bot);
            let plan = gear_plan(bot, &available);
            equip_plan(bot, &plan)?;
            return Ok(false);
        }
        Err(error) if crate::skill_common::is_walk_abort(&error) => return Err(error),
        Err(_) => {
            let available = available_gear(bot);
            let plan = gear_plan(bot, &available);
            equip_plan(bot, &plan)?;
            return Ok(false);
        }
    }
    let Some((booth_x, booth_z, booth_loc)) = crate::skill_common::live_bank_booth(bot, bank_x, bank_z) else {
        bot.log(&format!("bank did not open at {:?}", bot.position()));
        return Ok(false);
    };
    if !bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
        bot.log(&format!("bank did not open at {:?}", bot.position()));
        return Ok(false);
    }
    let bank_main = (bot.tables.com_id)("bank_main:inv")
        .ok_or_else(|| ScriptError::Usage("bank_main:inv missing from component table".to_string()))?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && !bot.world.inv.contains_key(&bank_main) {
        bot.pump(5)?;
    }
    if !bot.world.inv.contains_key(&bank_main) {
        bot.close_modal()?;
        return Ok(false);
    }
    let side_bank = (bot.tables.com_id)("bank_side:inv")
        .ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    let carried: Vec<(u16, u32)> = bot.inventory().into_iter().collect();
    let available = available_gear(bot);
    let mut keep: Vec<u16> = carried
        .iter()
        .filter(|(held, _)| {
            available.contains_key(held)
                && knowledge::equipment::all_wearables().iter().any(|entry| obj_id(bot, &entry.name).is_ok_and(|id| id == *held))
        })
        .map(|(held, _)| *held)
        .collect();
    let banked: Vec<String> = bot.bank_items().keys().filter_map(|id| (bot.tables.obj_name)(*id)).collect();
    let bank_refs: Vec<&str> = banked.iter().map(String::as_str).collect();
    if knowledge::food::best_food(&bank_refs).is_none() {
        keep.extend(carried.iter().filter(|(held, _)| {
            (bot.tables.obj_name)(*held).is_some_and(|name| knowledge::food::heal(&name) > 0)
        }).map(|(held, _)| *held));
    }
    keep.sort();
    keep.dedup();
    bot.bank_deposit_all(side_bank, &keep, 25)?;
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available);
    for slot in [3, 0, 4, 7, 5, 2, 9, 10, 12, 1] {
        for entry in plan.get(&slot).cloned().unwrap_or_default().into_iter().take(2) {
            let Ok(obj) = obj_id(bot, &entry.name) else {
                continue;
            };
            if bot.inventory().contains_key(&obj) || free_slots(bot) <= FOOD_TARGET {
                continue;
            }
            let _ = bot.bank_withdraw(obj, 1, 25);
        }
        if free_slots(bot) <= FOOD_TARGET {
            break;
        }
    }
    withdraw_food(bot)?;
    bot.close_modal()?;
    bot.pump(5)?;
    for (slot, name) in equip_plan(bot, &plan)?.iter() {
        bot.log(&format!("equipped {name} (slot {slot})"));
    }
    let _ = level;
    Ok(true)
}

/// NPC spawn clusters for a spot (same-level spawns only).
fn ranked_clusters(bot: &Bot, spot: &knowledge::combat_spots::CombatSpot) -> Vec<(i32, i32, usize)> {
    let mut spawns: Vec<(i64, i64)> = Vec::new();
    for entry in knowledge::all_npcs() {
        let name = entry.name.clone().unwrap_or_default();
        if !spot.npc_names.contains(&name.as_str()) || entry.members == Some(true) {
            continue;
        }
        for spawn in &entry.spawns {
            // shortcut: underground spots are unreachable with the
            // single-level router (walk_to never changes level); only
            // same-level spawns are fought.
            if spawn.level != i64::from(bot.world.level) {
                continue;
            }
            spawns.push((spawn.x, spawn.z));
        }
    }
    let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
    for spawn in spawns {
        let mut placed = false;
        for group in groups.iter_mut() {
            let center_x: i64 = group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64;
            let center_z: i64 = group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64;
            if (spawn.0 - center_x).abs().max((spawn.1 - center_z).abs()) <= 15 {
                group.push(spawn);
                placed = true;
                break;
            }
        }
        if !placed {
            groups.push(vec![spawn]);
        }
    }
    let mut ranked: Vec<(f64, i32, i32, usize)> = groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| {
            let center_x = (group.iter().map(|item| item.0).sum::<i64>() / group.len() as i64) as i32;
            let center_z = (group.iter().map(|item| item.1).sum::<i64>() / group.len() as i64) as i32;
            let dist = nearest_bank_tile(bot).map(|(_, bank_x, bank_z)| {
                (bank_x - center_x).abs() + (bank_z - center_z).abs()
            }).unwrap_or(10000) as f64;
            (group.len() as f64 - dist / 40.0, center_x, center_z, group.len())
        })
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().map(|(_, x, z, count)| (x, z, count)).collect()
}

fn attack_nearest(bot: &mut Bot, spot: &knowledge::combat_spots::CombatSpot) -> Result<Option<u16>, ScriptError> {
    let names: Vec<&str> = spot.npc_names.to_vec();
    let mut cands = bot.npcs_named(&names);
    cands.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
    let Some((slot, x, z)) = cands.first().copied() else {
        return Ok(None);
    };
    if (x - bot.world.x).abs() + (z - bot.world.z).abs() > 6 {
        // Dead socket, not a far foe: typed instead of Ok(None).
        match bot.approach(x, z, bot.world.level, 1, 1500) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(_) => return Ok(None),
        }
    }
    bot.op_npc(2, slot)?;
    Ok(Some(slot))
}

fn loot_ground(bot: &mut Bot) -> Result<u32, ScriptError> {
    let mut taken = 0u32;
    let mut items: Vec<((i32, i32, i32), u16)> = bot
        .world
        .ground_items(bot.world.x, bot.world.z, bot.world.level, LOOT_RADIUS, None)
        .into_iter()
        .map(|((tile, oid), _)| (tile, oid))
        .collect();
    items.sort_by_key(|(tile, _)| (tile.0 - bot.world.x).abs() + (tile.1 - bot.world.z).abs());
    for (_, oid) in items {
        if free_slots(bot) == 0 {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if name.is_empty() || name.to_lowercase().contains("bones") {
            continue;
        }
        // Dead socket, not a missed pickup: typed instead of skipping.
        match bot.take(Some(oid), None, 600) {
            Ok(true) => taken += 1,
            Ok(false) => {}
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(_) => {}
        }
    }
    bury_bones(bot)?;
    Ok(taken)
}

fn fight(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    target: (i32, i32),
    kills_limit: u32,
    ticks_limit: u64,
) -> Result<(u32, bool), ScriptError> {
    let mut kills = 0u32;
    let started = bot.world.ticks;
    let mut target_npc: Option<u16> = None;
    let mut last_attack = 0u64;
    let mut style_index = 0usize;
    let mut last_event_check = bot.world.ticks;
    let mut max_hp = hitpoints(bot).max(10);
    set_style(bot, MELEE_ORDER[style_index])?;
    loop {
        if ticks_limit > 0 && bot.world.ticks - started >= ticks_limit {
            break;
        }
        if kills_limit > 0 && kills >= kills_limit {
            break;
        }
        bot.pump(2)?;
        if bot.world.ticks - last_event_check >= 25 {
            last_event_check = bot.world.ticks;
            check_events(bot)?;
        }
        let hp = hitpoints(bot);
        if hp == 0 {
            bot.intend("dead, waiting for respawn");
            let target = bot.world.ticks + 40;
            while bot.world.ticks < target && hitpoints(bot) == 0 {
                bot.pump(5)?;
            }
            max_hp = hitpoints(bot).max(10);
            target_npc = None;
            continue;
        }
        max_hp = max_hp.max(hp);
        if f64::from(hp) <= f64::from(max_hp) * EAT_FRACTION {
            eat(bot)?;
        }
        if let Some(slot) = target_npc {
            if bot.world.npcs.contains_key(&slot) {
                if bot.world.ticks - last_attack >= 5 {
                    bot.op_npc(2, slot)?;
                    last_attack = bot.world.ticks;
                }
                continue;
            }
            kills += 1;
            bot.log(&format!("kill {kills} at {:?}", bot.position()));
            target_npc = None;
            loot_ground(bot)?;
            if kills % STYLE_KILLS == 0 {
                style_index = (style_index + 1) % MELEE_ORDER.len();
                set_style(bot, MELEE_ORDER[style_index])?;
            }
            if free_slots(bot) <= 1 || food_count(bot) == 0 {
                return Ok((kills, true));
            }
            continue;
        }
        target_npc = attack_nearest(bot, spot)?;
        if let Some(slot) = target_npc {
            last_attack = bot.world.ticks;
            let _ = slot;
        } else if (bot.world.x - target.0).abs() + (bot.world.z - target.1).abs() > 8 {
            // Dead socket, not an empty field: typed instead of pacing.
            if let Err(error) = bot.approach(target.0, target.1, bot.world.level, 1, 1500) {
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) {
                    return Err(error.into());
                }
            }
            wait_ticks(bot, 5)?;
        } else {
            wait_ticks(bot, 5)?;
        }
    }
    Ok((kills, false))
}

fn run_spot(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<u32, ScriptError> {
    if spot.entrance.is_some() {
        // shortcut: trapdoors/ladders to underground areas are not traversed
        // (single-level router); surface spots only.
        return Err(ScriptError::Usage(format!(
            "spot {} is underground, not traversable from {:?}",
            spot.key,
            bot.position()
        )));
    }
    let targets = ranked_clusters(bot, spot);
    if targets.is_empty() {
        bot.log(&format!("no spawn for {}", spot.key));
        return Ok(0);
    }
    let mut total = 0u32;
    for (center_x, center_z, count) in targets {
        // Pace retries: instant walk failures consume no ticks, so an
        // unpaced loop machine-guns the server into kicking us.
        bot.pump(2)?;
        bot.log(&format!("spot {} target ({center_x},{center_z}) spawns={count}", spot.key));
        while kills_limit == 0 || total < kills_limit {
            if !no_bank {
                prepare_at_bank(bot)?;
            } else {
                let available = available_gear(bot);
                equip_plan(bot, &gear_plan(bot, &available))?;
            }
            // A dead socket must reconnect, not limp to the next cluster:
            // propagate connection failures, swallow the rest as before.
            match walk_checked(bot, center_x, center_z, 15000) {
                Err(error) if crate::skill_common::is_conn_abort(&error) => return Err(error),
                Err(_) | Ok(false) => break,
                Ok(true) => {}
            }
            {
                let (kills, banked) = fight(bot, spot, (center_x, center_z), kills_limit - total, ticks_limit)?;
                total += kills;
                if !banked || (kills_limit > 0 && total >= kills_limit) {
                    return Ok(total);
                }
            }
        }
    }
    Ok(total)
}

/// Melee farmer: `melee [spot ...] [--kills N] [--ticks N] [--no-bank]`.
pub fn run_melee(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let mut spots: Vec<String> = Vec::new();
    let mut kills = 0u32;
    let mut ticks = 0u64;
    let mut no_bank = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--kills" => {
                kills = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--ticks" => {
                ticks = args.get(index + 1).and_then(|arg| arg.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--no-bank" => {
                no_bank = true;
                index += 1;
            }
            other => {
                spots.push(other.to_string());
                index += 1;
            }
        }
    }
    crate::skill_common::wait_pack_streamed(bot)?;
    let target = bot.world.ticks + 60;
    while bot.world.ticks < target && (!bot.world.stats.contains_key(&0) || !bot.world.stats.contains_key(&3)) {
        bot.pump(5)?;
    }
    check_events(bot)?;
    let picked: Vec<knowledge::combat_spots::CombatSpot> = {
        let named: Vec<_> = spots.iter().filter_map(|key| knowledge::combat_spots::by_key(key)).collect();
        if named.is_empty() {
            vec![knowledge::combat_spots::best_spot(melee_level(bot))]
        } else {
            named
        }
    };
    bot.log(&format!("melee={} spots={}", melee_level(bot), picked.iter().map(|spot| spot.key).collect::<Vec<_>>().join(",")));
    for spot in picked {
        bot.log(&format!("=== {} ===", spot.key));
        bot.intend(&format!("melee at {}", spot.key));
        let total = run_spot(bot, &spot, kills, ticks, no_bank)?;
        bot.log(&format!("ok {} killed {total}", spot.key));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::prepare_at_bank;
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// No socket: the bank walk fails typed (reconnect) instead of a quiet
    /// equip-and-continue that strands the fighter ungeared mid-field.
    #[test]
    fn prepare_offline_aborts_typed() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        bot.world.stats.insert(3, (10, 0));
        let error = prepare_at_bank(&mut bot).expect_err("dead socket aborts typed");
        assert!(crate::skill_common::is_walk_abort(&error), "typed for the reconnect arm, got: {error}");
    }
}
