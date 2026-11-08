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
/// World ticks a "can't reach" goblin tile stays banned before retry.
const BLOCKED_BAN_TICKS: u64 = 200;
/// Ban for a foe behind a door the server proved uncrossable (door opens,
/// the step through is refused): the tile stays out for the whole fight.
/// A spot that fails in a loop is worse than a spot skipped: each new
/// indoor goblin burns one approach, never a campaign.
const DOOR_BAN_TICKS: u64 = u64::MAX;
/// Worst-case take: the drop lands on the death tick (goblin.rs2 obj_add
/// right after npc_death), walk <= 6 tiles + click. 600 burned 12 s on every
/// vanished/unreachable item; failures now log so the bound stays honest.
const TAKE_TICKS: u64 = 100;
/// No combat xp and no steps for this long while the foe lives: not
/// fighting (fence, stale slot, single-way block). ~35 s at the measured
/// ~5.6 ticks/s; a real engagement lands xp within seconds of contact.
const FIGHT_STALL_TICKS: u64 = 200;
/// One foe never holds the bot longer than this, xp trickle or not: a normal
/// goblin kill costs <= ~600 ticks all-in (t01 live: kills 1-6), so 900 is
/// 1.5x headroom. The 12-minute goblin 810 grind (xp flowing, nothing dying)
/// is by definition a bad use of time on an axe rush.
const KILL_CAP_TICKS: u64 = 900;
const ARMOUR_SLOTS: [i64; 9] = [0, 1, 2, 4, 5, 7, 9, 10, 12];
/// Server wearpos slots: 3 is the weapon, 5 the shield.
const WEAPON_SLOT: i64 = 3;
const WORN_COM_NAME: &str = "wornitems:wear";

/// Controlled style button per weapon category, when this revision offers
/// one: slash/spiked/claws index 2, spear 0-2, polearm 0. Controlled trains
/// attack + strength + defence at once (combat.rs2:151-154, each 133/100 of
/// base xp). Unarmed has NO controlled entry (unarmed_table), so bare fists
/// rotate instead of sitting on accurate.
/// Source: content/scripts/skill_combat/configs/combat.dbrow (slash_table,
/// spear_table, polearm_table, spiked_table, claws_table) crossed with
/// content/scripts/skill_combat/scripts/player/player_attackstyles.rs2:37-76
/// (button suffix -> style index).
fn controlled_button(category: &str) -> Option<&'static str> {
    match category {
        "weapon_slash" => Some("combat_hacksword:hack2"),
        "weapon_spiked" => Some("combat_spiked:spiked2"),
        "weapon_claws" => Some("combat_claw:claw2"),
        "weapon_spear" => Some("combat_spear:spear0"),
        "weapon_polearm" => Some("combat_polearm:polearm0"),
        _ => None,
    }
}

/// Combat xp across the four fighting stats (attack 0, defence 1, strength 2,
/// hitpoints 3): any landed hit moves it, so a flat line means no combat.
fn combat_xp(bot: &Bot) -> i64 {
    [0u8, 1, 2, 3].iter().filter_map(|index| bot.world.stats.get(index)).map(|(_, xp)| i64::from(*xp)).sum()
}

/// Level of one melee skill by stat id (0 attack, 1 defence, 2 strength).
fn melee_stat(bot: &Bot, skill: &str) -> i64 {
    let index = match skill {
        "attack" => 0u8,
        "defence" => 1u8,
        _ => 2u8,
    };
    bot.world.stats.get(&index).map(|(level, _)| i64::from(*level)).unwrap_or(1)
}

/// Style to switch to, if any: controlled never rotates, otherwise the
/// weakest skill when it sits a full level below the current one (ties and
/// balanced skills hold, no click spam).
fn next_style(bot: &Bot, current: &str) -> Option<&'static str> {
    if current == "controlled" {
        return None;
    }
    let want = weakest_melee(bot);
    (want != current && melee_stat(bot, want) < melee_stat(bot, current)).then_some(want)
}

/// Weakest of attack/strength/defence right now.
fn weakest_melee(bot: &Bot) -> &'static str {
    let (attack, strength, defence) =
        (melee_stat(bot, "attack"), melee_stat(bot, "strength"), melee_stat(bot, "defence"));
    if attack <= strength && attack <= defence {
        "attack"
    } else if strength <= defence {
        "strength"
    } else {
        "defence"
    }
}

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

/// Attack bonus the bot's next button actually trains with, per weapon
/// category: the revision ties each combat button to one damagetype
/// (slash/stab/crush), and the bot presses the button for the skill it
/// trains (style_buttons) or the controlled button when the category
/// offers one. Unknown pairs fall back to the best bonus.
/// Sources (read-only): content/scripts/skill_combat/configs/combat.dbrow
/// (weapon_*_table damagestyle/damagetype rows: slash_table is
/// acc/agg/con/def on slash/slash/stab/slash, stab_table acc/agg/agg/def
/// on stab/stab/slash/stab, blunt_table all crush, axe_table
/// acc/agg/agg/def on slash/slash/crush/slash, pickaxe_table on
/// stab/stab/crush/stab, spear_table all controlled on
/// stab/slash/crush/stab, spiked_table acc/agg/con/def on
/// crush/crush/stab/crush, claws_table like slash, polearm_table
/// con/agg/def on stab/slash/stab, unarmed/staff all crush) crossed with
/// content/.../player/player_attackstyles.rs2 (button suffix -> style).
fn style_damage_key(category: &str, style: &str) -> Option<&'static str> {
    match (category, style) {
        ("weapon_slash" | "weapon_claws", "controlled") => Some("stab"),
        ("weapon_slash" | "weapon_claws", _) => Some("slash"),
        ("weapon_stab", _) => Some("stab"),
        ("weapon_blunt", _) => Some("crush"),
        ("weapon_axe" | "weapon_2h_sword", _) => Some("slash"),
        ("weapon_pickaxe", _) => Some("stab"),
        ("weapon_spear", _) => Some("stab"),
        ("weapon_spiked", "controlled") => Some("stab"),
        ("weapon_polearm", "controlled") => Some("stab"),
        ("weapon_polearm", "defence") => Some("stab"),
        ("weapon_polearm", _) => Some("slash"),
        ("unarmed", _) => Some("crush"),
        _ => None,
    }
}

/// Replacement cost of a wearable (shop price from the objs dump, 0 when
/// the dump carries none, unknown names last): the tie-break when two
/// items score equal is the cheaper one to lose on a death far from a
/// bank. No gender gate exists in this revision (equipment.json
/// requirements only ever name attack/defence/strength).
fn loss_cost(name: &str) -> i64 {
    knowledge::objs_by_name(name).first().map(|entry| entry.price).unwrap_or(i64::MAX)
}

/// Gear score, slot by slot. Weapon: the style's own attack bonus plus
/// twice the strength bonus (expected DPS at the current style), scaled
/// by the category speed proxy. Armour: summed defence plus twice
/// strength plus prayer (survival). Level gates come from
/// equipment.json requirements via requirements_met (wield gates in
/// content/scripts/levelrequire/scripts/tier1|5|10|20|30|40.rs2:
/// iron attack 1, steel 5, black 10, mithril 20, adamant 30, rune 40 —
/// bronze ungated outside the tutorial); ties keep the cheaper kit.
fn gear_score(bot: &Bot, entry: &knowledge::equipment::Wearable, slot: i64) -> f64 {
    if slot == WEAPON_SLOT {
        let category = entry.category.as_deref().unwrap_or("");
        // Controlled trains everything when the category offers it,
        // otherwise the weakest skill right now: the same button fight()
        // will press, so the bonus scored is the bonus trained with.
        let style = if controlled_button(category).is_some() { "controlled" } else { weakest_melee(bot) };
        let attack = match style_damage_key(category, style) {
            Some(key) => entry.attack.iter().find(|(name, _)| name == key).map(|(_, bonus)| *bonus).unwrap_or(0),
            None => entry.attack.iter().map(|(_, bonus)| *bonus).max().unwrap_or(0),
        } as f64;
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
        // Best score wins, ties keep the cheaper kit (death far from a
        // bank drops the pack: risk without a bonus is just cost).
        cands.sort_by(|a, b| {
            gear_score(bot, b, *slot)
                .partial_cmp(&gear_score(bot, a, *slot))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| loss_cost(&a.name).cmp(&loss_cost(&b.name)))
                .then_with(|| a.name.cmp(&b.name))
        });
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
    let dotted = if skill == "controlled" {
        controlled_button(&category)
    } else {
        style_buttons(&category, skill)
    };
    if let Some(dotted) = dotted {
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
            // shortcut: same-level spawns only; the fighter never rides
            // down to a dungeon camp. Revisit if one is wanted.
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

/// Toll-fence side test: the Al-Kharid gate (locs 2882/2883, two halves;
/// nav/tests/paid_only.rs measures the foot detour at 220 tiles) costs 10
/// coins a crossing with paid doors on. Melee keeps them off the whole
/// hunt, so a crossing is always the 220-tile walk, never a payment.
/// Same side when both tiles sit on the same flank of the gate line.
fn same_fence_side(line: i32, first: i32, second: i32) -> bool {
    (first >= line) == (second >= line)
}

/// Shop and bank staff are never targets: keepers, assistants, Bob and
/// bankers (some at 1 HP) must survive our hunts. Excluded by op
/// category — any Trade/Bank right-click — never by name, so a new shop
/// inherits the guard. Valid foes carry Attack instead (goblin npc 100:
/// [None, Attack, None, ...]).
/// Source (read-only): gamedata npcs.json ops, e.g. generalshopkeeper1
/// (npc 520) and generalassistant1 (521) carry Trade at op index 2.
fn is_shop_staff(npc_type: u16) -> bool {
    knowledge::npc_by_id(usize::from(npc_type)).is_some_and(|entry| {
        entry.ops.iter().flatten().any(|op| {
            let lowered = op.to_lowercase();
            lowered.contains("trade") || lowered.contains("bank")
        })
    })
}

fn attack_nearest(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    avoid: &mut HashMap<(i32, i32), u64>,
) -> Result<Option<u16>, ScriptError> {
    let names: Vec<&str> = spot.npc_names.to_vec();
    let mut cands = bot.npcs_named(&names);
    cands.retain(|(_, x, z)| avoid.get(&(*x, *z)).is_none_or(|until| *until <= bot.world.ticks));
    // Shop staff are never foes, by op category (Trade/Bank), not name:
    // a keeper next to the hunt (Bob by the axe shop, the Lumbridge
    // general store pair) is never a valid click.
    cands.retain(|(slot, _, _)| {
        bot.world.npcs.get(slot).is_none_or(|npc| !is_shop_staff(npc.npc_type))
    });
    // Never target across the toll fence: goblins spawn on both flanks,
    // so the nearest flips sides every kill and each crossing walks the
    // ~200-tile foot detour (paid doors stay off all hunt). One side per
    // pick; the flank follows the bot, not the plan, so no replan loop.
    if let Some(line) = crate::skill_common::toll_gate_x(bot) {
        let home = bot.world.x;
        cands.retain(|(_, x, _)| same_fence_side(line, *x, home));
    }
    cands.sort_by_key(|(_, x, z)| (x - bot.world.x).abs() + (z - bot.world.z).abs());
    // Door-aware acquisition, first reachable wins (max 3 foes): stand
    // orthogonally adjacent via the router, which opens shut doors on the
    // way (travel Segment::OpenDoor). NoRoute or a blocked ring bans the
    // tile and tries the next goblin instead of insisting on this one.
    for (slot, x, z) in cands.into_iter().take(3) {
        match bot.approach_npc(slot, 100) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => {
                // Uncrossable door (verified open, step refused): the house
                // is out for this fight, not the foe. One approach burned
                // per tile, never a retry campaign.
                if matches!(error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                    bot.log(&format!("goblin {slot} at ({x},{z}) behind uncrossable door, excluding: {error}"));
                    avoid.insert((x, z), DOOR_BAN_TICKS);
                    continue;
                }
                bot.log(&format!("goblin {slot} at ({x},{z}) unreachable: {error}"));
                avoid.insert((x, z), bot.world.ticks + BLOCKED_BAN_TICKS);
                continue;
            }
        }
        bot.op_npc(2, slot)?;
        return Ok(Some(slot));
    }
    Ok(None)
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
    // Coins first: the goblin bootstrap exists to fund the axe, junk never
    // starves the coin pickup when the pack is nearly full.
    let coins = (bot.tables.find_obj_by_name)("coins");
    items.sort_by_key(|(_, oid)| if Some(*oid) == coins { 0 } else { 1 });
    for (_, oid) in items {
        if free_slots(bot) == 0 {
            break;
        }
        let name = (bot.tables.obj_name)(oid).unwrap_or_default();
        if name.is_empty() || name.to_lowercase().contains("bones") {
            continue;
        }
        // Dead socket, not a missed pickup: typed instead of skipping.
        match bot.take(Some(oid), None, TAKE_TICKS) {
            Ok(true) => taken += 1,
            Ok(false) => bot.log(&format!("loot {name} gave nothing at {:?}", bot.position())),
            Err(error)
                if matches!(
                    error,
                    lifescape_sdk::Error::Interrupted(_) | lifescape_sdk::Error::Io(_) | lifescape_sdk::Error::Login(_)
                ) =>
            {
                return Err(error.into())
            }
            Err(error) => bot.log(&format!("loot {name} failed at {:?}: {error}", bot.position())),
        }
    }
    bury_bones(bot)?;
    Ok(taken)
}

/// Re-wear the best kit from what is actually carried (pack + worn):
/// after a death the pack dropped, after a loot the best changed. One
/// evaluation per event, never per kill or per tick; no bank trip and
/// never a buy — the bank only enters at prepare_at_bank on the way in.
/// Logs the slots that changed, quiet when nothing does.
fn rewear(bot: &mut Bot, why: &str) -> Result<(), ScriptError> {
    let before: HashMap<i64, u16> = ARMOUR_SLOTS
        .into_iter()
        .chain([WEAPON_SLOT])
        .filter_map(|slot| worn_obj(bot, slot).map(|obj| (slot, obj)))
        .collect();
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available);
    equip_plan(bot, &plan)?;
    for slot in ARMOUR_SLOTS.into_iter().chain([WEAPON_SLOT]) {
        if worn_obj(bot, slot) != before.get(&slot).copied() {
            let name =
                worn_obj(bot, slot).and_then(|obj| (bot.tables.obj_name)(obj)).unwrap_or_default();
            bot.log(&format!("{why}: now wearing {name} (slot {slot})"));
        }
    }
    Ok(())
}

fn fight(
    bot: &mut Bot,
    spot: &knowledge::combat_spots::CombatSpot,
    target: (i32, i32),
    kills_limit: u32,
    ticks_limit: u64,
    no_bank: bool,
) -> Result<(u32, bool), ScriptError> {
    let mut kills = 0u32;
    let started = bot.world.ticks;
    let mut target_npc: Option<u16> = None;
    let mut last_attack = 0u64;
    let mut last_event_check = bot.world.ticks;
    let mut max_hp = hitpoints(bot).max(10);
    let mut seen_msgs = bot.world.messages.len();
    let mut unreachable: HashMap<(i32, i32), u64> = HashMap::new();
    let mut progress_at = bot.world.ticks;
    let mut progress_xp = combat_xp(bot);
    let mut progress_pos = (bot.world.x, bot.world.z);
    let mut engaged_at = bot.world.ticks;
    let mut attempts = 0u32;
    // Controlled (all three at once) when the wielded category offers it,
    // else the weakest skill right now: unarmed has no controlled entry.
    let mut style: String = if controlled_button(&current_weapon_category(bot)).is_some() {
        "controlled".to_string()
    } else {
        weakest_melee(bot).to_string()
    };
    set_style(bot, &style)?;
    bot.log(&format!(
        "style {style} (atk={} str={} def={})",
        melee_stat(bot, "attack"),
        melee_stat(bot, "strength"),
        melee_stat(bot, "defence")
    ));
    // One line per hunt: which flank we hold (or that no gate is mapped
    // yet, so the per-pick filter stays off and crossings stay possible).
    match crate::skill_common::toll_gate_x(bot) {
        Some(line) => bot.log(&format!("toll fence line {line}, hunting the {} flank", if bot.world.x >= line { "east" } else { "west" })),
        None => bot.log("toll fence: no gates mapped, cross-fence picks unfiltered"),
    }
    loop {
        if ticks_limit > 0 && bot.world.ticks.saturating_sub(started) >= ticks_limit {
            break;
        }
        if kills_limit > 0 && kills >= kills_limit {
            break;
        }
        bot.pump(2)?;
        if bot.world.ticks.saturating_sub(last_event_check) >= 25 {
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
            attempts = 0;
            progress_at = bot.world.ticks;
            // Death drops the pack: re-wear from what survived, pack only,
            // never a buy loop.
            rewear(bot, "death")?;
            continue;
        }
        max_hp = max_hp.max(hp);
        if f64::from(hp) <= f64::from(max_hp) * EAT_FRACTION {
            eat(bot)?;
        }
        if let Some(slot) = target_npc {
            if bot.world.npcs.contains_key(&slot) {
                // One foe never holds the bot past the cap, xp trickle or
                // not: ban, step toward it (door-aware), repick. A target at
                // 1 hp retried after the ban expiry, nothing is lost.
                if bot.world.ticks.saturating_sub(engaged_at) >= KILL_CAP_TICKS {
                    let (foe_x, foe_z) =
                        bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z)).unwrap_or((bot.world.x, bot.world.z));
                    bot.log(&format!("target {slot} alive {}t, moving on", bot.world.ticks.saturating_sub(engaged_at)));
                    unreachable.insert((foe_x, foe_z), bot.world.ticks + BLOCKED_BAN_TICKS);
                    let _ = bot.approach_npc(slot, 60);
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                let blocked = bot.world.messages.get(seen_msgs..).unwrap_or(&[]).iter().any(|line| {
                    // Server refusals (player_combat.rs2 player_in_combat_check)
                    // or unreachable foe: repick instead of spamming the same
                    // click every 5 ticks forever (t01 baseline: 229 "can't
                    // reach" + 36 "already under attack" for 26 kills).
                    let lowered = line.to_lowercase();
                    lowered.contains("can't reach that")
                        || lowered.contains("already under attack")
                        || lowered.contains("someone else is fighting")
                        || lowered.contains("not after you")
                });
                let (foe_x, foe_z) =
                    bot.world.npcs.get(&slot).map(|npc| (npc.x, npc.z)).unwrap_or((bot.world.x, bot.world.z));
                if blocked {
                    unreachable.insert((foe_x, foe_z), bot.world.ticks + BLOCKED_BAN_TICKS);
                    // The step toward a door-blocked foe re-burns the door
                    // (click + watch, refused again): inspect it, and when
                    // the server proves the door uncrossable the tile stays
                    // out for the whole fight instead of 200 ticks.
                    if let Err(step_error) = bot.approach_npc(slot, 60) {
                        if matches!(step_error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                            bot.log(&format!("npc {slot} at ({foe_x},{foe_z}) behind uncrossable door, excluding"));
                            unreachable.insert((foe_x, foe_z), DOOR_BAN_TICKS);
                        }
                    }
                    seen_msgs = bot.world.messages.len();
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                // Door-aware step: orthogonal adjacency via the router, which
                // opens shut doors on the way (goblin in a house, or bot
                // locked inside: the walk crosses the door instead of
                // spamming clicks). 3 failed bouts blacklist the foe.
                match bot.approach_npc(slot, 100) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error,
                            lifescape_sdk::Error::Interrupted(_)
                                | lifescape_sdk::Error::Io(_)
                                | lifescape_sdk::Error::Login(_)
                        ) =>
                    {
                        return Err(error.into())
                    }
                    Err(error) => {
                        // Door proved uncrossable: no 3-bout retry (each bout
                        // is a click + watch the server refuses). Exclude the
                        // tile for the fight and repick at once.
                        if matches!(error, lifescape_sdk::Error::DoorDidNotOpen(_)) {
                            bot.log(&format!("npc {slot} at ({foe_x},{foe_z}) behind uncrossable door, excluding: {error}"));
                            unreachable.insert((foe_x, foe_z), DOOR_BAN_TICKS);
                            target_npc = None;
                            attempts = 0;
                            progress_at = bot.world.ticks;
                            continue;
                        }
                        attempts += 1;
                        bot.log(&format!("npc {slot} approach {attempts}/3 failed: {error}"));
                        if attempts >= 3 {
                            bot.log(&format!(
                                "blacklist npc {slot} at ({foe_x},{foe_z}): 3 failed approaches from {:?}",
                                bot.position()
                            ));
                            unreachable.insert((foe_x, foe_z), bot.world.ticks + BLOCKED_BAN_TICKS);
                            target_npc = None;
                            attempts = 0;
                            progress_at = bot.world.ticks;
                        }
                        continue;
                    }
                }
                // No xp and no steps for a whole stall window while the foe
                // lives: not fighting (locked in/out around a door the
                // router could not cross). Ban, step out toward the foe,
                // repick. Attempts reset on xp only: circling a blocked
                // house is movement, not progress, or 3 failed bouts would
                // never blacklist.
                if combat_xp(bot) != progress_xp {
                    progress_xp = combat_xp(bot);
                    progress_pos = (bot.world.x, bot.world.z);
                    progress_at = bot.world.ticks;
                    attempts = 0;
                } else if (bot.world.x, bot.world.z) != progress_pos {
                    progress_pos = (bot.world.x, bot.world.z);
                    progress_at = bot.world.ticks;
                } else if bot.world.ticks.saturating_sub(progress_at) >= FIGHT_STALL_TICKS {
                    bot.log(&format!("stall on npc {slot}, repicking"));
                    unreachable.insert((foe_x, foe_z), bot.world.ticks + BLOCKED_BAN_TICKS);
                    let _ = bot.approach_npc(slot, 60);
                    target_npc = None;
                    attempts = 0;
                    progress_at = bot.world.ticks;
                    continue;
                }
                if bot.world.ticks.saturating_sub(last_attack) >= 5 {
                    bot.op_npc(2, slot)?;
                    last_attack = bot.world.ticks;
                }
                continue;
            }
            kills += 1;
            let loot_from = bot.world.ticks;
            let taken = loot_ground(bot)?;
            bot.log(&format!("kill {kills} at {:?} (+{}t loot)", bot.position(), bot.world.ticks.saturating_sub(loot_from)));
            // Loot can change the best: one re-evaluation when something
            // was picked up, never on empty kills.
            if taken > 0 {
                rewear(bot, "loot")?;
            }
            seen_msgs = bot.world.messages.len();
            progress_at = bot.world.ticks;
            attempts = 0;
            target_npc = None;
            if let Some(want) = next_style(bot, &style) {
                style = want.to_string();
                set_style(bot, &style)?;
                bot.log(&format!(
                    "style -> {style} (atk={} str={} def={})",
                    melee_stat(bot, "attack"),
                    melee_stat(bot, "strength"),
                    melee_stat(bot, "defence")
                ));
            }
            if free_slots(bot) <= 1 || (!no_bank && food_count(bot) == 0) {
                return Ok((kills, true));
            }
            continue;
        }
        unreachable.retain(|_, until| *until > bot.world.ticks);
        target_npc = attack_nearest(bot, spot, &mut unreachable)?;
        if let Some(slot) = target_npc {
            last_attack = bot.world.ticks;
            engaged_at = bot.world.ticks;
            attempts = 0;
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

/// Clusters on the bot's toll-fence flank (see same_fence_side): the
/// crossing is never paid in melee, so an eastern cluster from a western
/// hunt is a ~220-tile detour per visit, never a shortcut. Falls back to
/// every cluster when none shares the flank (an all-across spot still
/// hunts instead of idling).
fn same_flank_clusters(line: i32, home: i32, targets: &[(i32, i32, usize)]) -> Vec<(i32, i32, usize)> {
    let kept: Vec<(i32, i32, usize)> =
        targets.iter().filter(|(center_x, _, _)| same_fence_side(line, *center_x, home)).copied().collect();
    if kept.is_empty() {
        targets.to_vec()
    } else {
        kept
    }
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
    // One flank per hunt: cluster centers across the toll fence would
    // each cost the ~220-tile foot detour (paid doors stay off all
    // hunt). The per-pick filter in attack_nearest stops the same
    // oscillation inside the fight; this stops it between clusters.
    let mut targets = targets;
    if let Some(line) = crate::skill_common::toll_gate_x(bot) {
        let kept = same_flank_clusters(line, bot.world.x, &targets);
        if kept.len() != targets.len() {
            bot.log(&format!("toll fence line {line}: hunting {} of {} clusters on this flank", kept.len(), targets.len()));
        }
        targets = kept;
    }
    let mut total = 0u32;
    for (center_x, center_z, count) in targets {
        // Pace retries: instant walk failures consume no ticks, so an
        // unpaced loop machine-guns the server into kicking us.
        bot.pump(2)?;
        bot.log(&format!("spot {} target ({center_x},{center_z}) spawns={count}", spot.key));
        while kills_limit == 0 || total < kills_limit {
            // Hunt entry: the one full evaluation (pack + bank when near
            // one, pack only with --no-bank). Inside the hunt only a loot
            // that was picked up or a death re-evaluates (see fight()).
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
                let (kills, banked) = fight(bot, spot, (center_x, center_z), kills_limit - total, ticks_limit, no_bank)?;
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
    use super::{controlled_button, gear_score, loss_cost, melee_stat, next_style, prepare_at_bank, style_damage_key, weakest_melee};
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    fn stat_bot(attack: u8, defence: u8, strength: u8) -> Bot {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        bot.world.stats.insert(0, (attack, 0));
        bot.world.stats.insert(1, (defence, 0));
        bot.world.stats.insert(2, (strength, 0));
        bot
    }

    /// t01's live state (atk 10, str/def 1): the laggard trains, never the
    /// leader. Previously the blind 3-kill rotation sat on accurate.
    #[test]
    fn weakest_melee_picks_laggard() {
        let bot = stat_bot(10, 1, 1);
        assert_eq!(weakest_melee(&bot), "strength");
        assert_eq!(melee_stat(&bot, "attack"), 10);
        assert_eq!(next_style(&bot, "attack"), Some("strength"));
        assert_eq!(next_style(&bot, "strength"), None, "already training the weakest: hold");
        assert_eq!(next_style(&bot, "controlled"), None, "controlled never rotates");
    }

    /// Balanced skills hold the current style (no click spam on ties), a
    /// one-level gap rotates to the laggard.
    #[test]
    fn rotation_holds_ties_switches_on_gap() {
        let tied = stat_bot(5, 5, 5);
        assert_eq!(next_style(&tied, "attack"), None);
        let gapped = stat_bot(6, 5, 5);
        assert_eq!(next_style(&gapped, "attack"), Some("strength"), "5 < 6 trains first");
        assert_eq!(next_style(&gapped, "strength"), None, "weakest held");
    }

    /// Controlled exists exactly where the revision's combat.dbrow puts a
    /// ^style_melee_controlled entry (slash/spear/polearm/spiked/claws);
    /// unarmed and plain axes have none.
    #[test]
    fn controlled_only_where_the_revision_has_it() {
        assert_eq!(controlled_button("weapon_slash"), Some("combat_hacksword:hack2"));
        assert_eq!(controlled_button("weapon_spear"), Some("combat_spear:spear0"));
        assert_eq!(controlled_button("weapon_polearm"), Some("combat_polearm:polearm0"));
        assert_eq!(controlled_button("unarmed"), None);
        assert_eq!(controlled_button("weapon_axe"), None);
        assert_eq!(controlled_button("weapon_blunt"), None);
    }

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

    fn slash_weapon(slash: i64, stab: i64) -> lifescape_gamedata::equipment::Wearable {
        lifescape_gamedata::equipment::Wearable {
            name: "test_slash".to_string(),
            wearpos: 3,
            wearpos2: -1,
            wearpos3: -1,
            category: Some("weapon_slash".to_string()),
            attack: vec![("stab".to_string(), stab), ("slash".to_string(), slash)],
            defence: Vec::new(),
            strength: 0,
            prayer: 0,
            requirements: Vec::new(),
        }
    }

    /// Style bonus, not max bonus: a slash weapon on controlled trains
    /// stab (dbrow slash_table button 2), so the stab-heavy blade wins
    /// even when its slash is worse.
    #[test]
    fn weapon_score_uses_the_trained_bonus() {
        let bot = stat_bot(5, 5, 5);
        let slashy = slash_weapon(8, 0);
        let stabby = slash_weapon(0, 8);
        assert_eq!(style_damage_key("weapon_slash", "controlled"), Some("stab"));
        assert_eq!(style_damage_key("weapon_slash", "attack"), Some("slash"));
        assert!(
            gear_score(&bot, &stabby, super::WEAPON_SLOT) > gear_score(&bot, &slashy, super::WEAPON_SLOT),
            "controlled trains stab: the stab blade must win"
        );
    }

    /// Style map covers the buttons the bot presses (dbrow damagetypes x
    /// player_attackstyles suffixes); unknown categories fall back to max.
    #[test]
    fn style_damage_follows_the_pressed_button() {
        assert_eq!(style_damage_key("weapon_stab", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_blunt", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_axe", "strength"), Some("slash"));
        assert_eq!(style_damage_key("weapon_pickaxe", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_spear", "attack"), Some("stab"));
        assert_eq!(style_damage_key("weapon_polearm", "controlled"), Some("stab"));
        assert_eq!(style_damage_key("unarmed", "defence"), Some("crush"));
        assert_eq!(style_damage_key("weapon_bow", "attack"), None, "ranged: max fallback");
    }

    /// Equal scores keep the cheaper kit: bronze before steel (dump shop
    /// prices 26 vs 325), unknown names last.
    #[test]
    fn ties_keep_the_cheap_kit() {
        assert!(loss_cost("bronze_sword") < loss_cost("steel_sword"), "bronze cheaper to lose");
        assert_eq!(loss_cost("no_such_item_xyz"), i64::MAX, "unknown prices never win ties");
    }

    /// Toll-fence side test: the gate line splits flanks (Al-Kharid gate
    /// halves 2882/2883); same flank hunts together, never across.
    #[test]
    fn fence_side_splits_at_the_gate_line() {
        assert!(super::same_fence_side(3268, 3262, 3255), "west flank together");
        assert!(super::same_fence_side(3268, 3270, 3300), "east flank together");
        assert!(!super::same_fence_side(3268, 3262, 3270), "across never targets");
        assert!(!super::same_fence_side(3268, 3270, 3262), "across either way");
    }

    /// Shop staff are excluded by op category, never by name: the
    /// Lumbridge general store pair (520/521), Bob (519) and bankers
    /// carry Trade/Bank; every valid foe (goblin 100, chicken 41, man 1)
    /// carries Attack instead.
    #[test]
    fn shop_staff_never_targeted_by_category() {
        assert!(super::is_shop_staff(520), "Lumbridge keeper");
        assert!(super::is_shop_staff(521), "Lumbridge assistant");
        assert!(super::is_shop_staff(519), "Bob the axe seller");
        assert!(super::is_shop_staff(494), "banker");
        assert!(!super::is_shop_staff(100), "goblin fights");
        assert!(!super::is_shop_staff(41), "chicken fights");
        assert!(!super::is_shop_staff(1), "man fights");
    }

    /// Cluster flank filter: mixed flanks keep the bot's side only, an
    /// all-across list falls back to everything (still hunts).
    #[test]
    fn clusters_stay_on_the_hunting_flank() {
        let mixed = vec![(3252, 3243, 16), (3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3260, &mixed), vec![(3252, 3243, 16)], "east cluster dropped");
        assert_eq!(super::same_flank_clusters(3268, 3270, &mixed), vec![(3274, 3213, 4)], "west cluster dropped");
        let east_only = vec![(3274, 3213, 4)];
        assert_eq!(super::same_flank_clusters(3268, 3260, &east_only), east_only, "all-across still hunts");
    }
}
