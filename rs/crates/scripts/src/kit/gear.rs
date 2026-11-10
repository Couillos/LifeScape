//! Gear: best-kit planning, equipping and bank-backed restock. Moved out
//! of the melee script: "s'équiper au mieux" is a graph task
//! (`equip_best`), executed here. Rewear after death or loot never buys
//! and never bank-trips — the bank only enters at `prepare_at_bank` on
//! the way in.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{heal_at_bank, live_bank_booths, nearest_bank};
use crate::kit::pack::{free_slots, obj_id};
use crate::skill_walk::walk_checked;
use crate::ScriptError;

pub const ARMOUR_SLOTS: [i64; 9] = [0, 1, 2, 4, 5, 7, 9, 10, 12];
/// Server wearpos slots: 3 is the weapon, 5 the shield.
pub const WEAPON_SLOT: i64 = 3;
pub const WORN_COM_NAME: &str = "wornitems:wear";

/// Death-risk price of an item (dump shop price; unknown never wins ties).
pub fn loss_cost(name: &str) -> i64 {
    knowledge::objs_by_name(name).first().map(|entry| entry.price).unwrap_or(i64::MAX)
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

/// Mean of attack/strength/defence right now (hunt difficulty).
pub fn melee_level(bot: &Bot) -> i64 {
    let at = |index: u8| bot.world.stats.get(&index).map(|(level, _)| i64::from(*level)).unwrap_or(1);
    (at(0) + at(2) + at(1)) / 3
}

/// Display names of worn + banked + carried gear, with their obj ids.
pub fn available_gear(bot: &Bot) -> HashMap<u16, String> {
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

/// Best wearable per slot among the available obj ids. The style used for
/// the weapon score is the one `fight` will press (controlled when the
/// category offers it, else the weakest skill): `kit::fight` owns the
/// rotation, this only scores through the callback.
pub fn gear_plan(
    bot: &Bot,
    available: &HashMap<u16, String>,
    weapon_style: impl Fn(&Bot, &str) -> &'static str,
    style_damage_key: impl Fn(&str, &str) -> Option<&'static str>,
) -> HashMap<i64, Vec<knowledge::equipment::Wearable>> {
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
            gear_score(bot, b, *slot, &weapon_style, &style_damage_key)
                .partial_cmp(&gear_score(bot, a, *slot, &weapon_style, &style_damage_key))
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

/// Gear score, slot by slot. Weapon: the style's own attack bonus plus
/// twice the strength bonus (expected DPS at the current style), scaled
/// by the category speed proxy. Armour: summed defence plus twice
/// strength plus prayer (survival). Level gates come from
/// equipment.json requirements via requirements_met (wield gates in
/// content/scripts/skill_combat/configs/levelrequire/scripts/tier1|5|10|20|30|40.rs2:
/// iron attack 1, steel 5, black 10, mithril 20, adamant 30, rune 40 —
/// bronze ungated outside the tutorial); ties keep the cheaper kit.
fn gear_score(
    bot: &Bot,
    entry: &knowledge::equipment::Wearable,
    slot: i64,
    weapon_style: impl Fn(&Bot, &str) -> &'static str,
    style_damage_key: impl Fn(&str, &str) -> Option<&'static str>,
) -> f64 {
    if slot == WEAPON_SLOT {
        let category = entry.category.as_deref().unwrap_or("");
        // Controlled trains everything when the category offers it,
        // otherwise the weakest skill right now: the same button fight()
        // will press, so the bonus scored is the bonus trained with.
        let style = weapon_style(bot, category);
        let attack = match style_damage_key(category, style) {
            Some(key) => entry.attack.iter().find(|(name, _)| *name == key).map(|(_, bonus)| *bonus).unwrap_or(0),
            None => entry.attack.iter().map(|(_, bonus)| *bonus).max().unwrap_or(0),
        } as f64;
        return (attack + 2.0 * entry.strength as f64) * crate::kit::style::weapon_weight(category);
    }
    entry.defence.iter().map(|(_, bonus)| *bonus).sum::<i64>() as f64 + 2.0 * entry.strength as f64
        + entry.prayer as f64
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

pub fn equip_plan(bot: &mut Bot, plan: &HashMap<i64, Vec<knowledge::equipment::Wearable>>) -> Result<HashMap<i64, String>, ScriptError> {
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

pub fn current_weapon_category(bot: &Bot) -> String {
    match worn_obj(bot, WEAPON_SLOT) {
        Some(obj) => knowledge::equipment::all_wearables()
            .into_iter()
            .find(|entry| obj_id(bot, &entry.name).is_ok_and(|id| id == obj))
            .and_then(|entry| entry.category)
            .unwrap_or_else(|| "unarmed".to_string()),
        None => "unarmed".to_string(),
    }
}

/// Deposit loot, withdraw the best gear and food for the current levels.
pub fn prepare_at_bank(bot: &mut Bot) -> Result<bool, ScriptError> {
    let bank = match nearest_bank(bot) {
        // No service means no fence line and no walk: typed `nav-down`
        // stays loud instead of degrading into an equip-and-continue.
        Err(error)
            if matches!(&error, ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")) =>
        {
            return Err(error)
        }
        Ok(bank) => bank,
        Err(_) => {
            let available = available_gear(bot);
            let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
            equip_plan(bot, &plan)?;
            return Ok(false);
        }
    };
    let (bank_x, bank_z) = (bank.x as i32, bank.z as i32);
    bot.intend(&format!("banking at ({bank_x},{bank_z})"));
    // Dead socket or operator stop, not a missed bank: typed to the
    // session instead of a quiet equip-and-continue.
    match walk_checked(bot, bank_x, bank_z, 15000) {
        Ok(true) => {}
        Ok(false) => {
            let available = available_gear(bot);
            let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
            equip_plan(bot, &plan)?;
            return Ok(false);
        }
        // No service means no walk at all: typed `nav-down` stays loud
        // instead of degrading into an equip-and-continue.
        Err(error)
            if matches!(&error, ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")) =>
        {
            return Err(error)
        }
        Err(error) if crate::kit::survival::is_walk_abort(&error) => return Err(error),
        Err(_) => {
            let available = available_gear(bot);
            let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
            equip_plan(bot, &plan)?;
            return Ok(false);
        }
    }
    // The bank tile is safe: heal low HP here before the booth instead
    // of walking hurt into the next hunt.
    heal_at_bank(bot)?;
    // Nearest booth first, every refusal falls through to the next: the
    // dump's Manhattan pick can name a spawn behind the counter while its
    // neighbour faces us. Only all-refused is quiet.
    let mut opened = false;
    for (booth_x, booth_z, booth_loc) in live_bank_booths(bot, bank_x, bank_z) {
        if bot.open_bank(booth_x, booth_z, booth_loc, 50)? {
            opened = true;
            break;
        }
        bot.log(&format!("bank booth ({booth_x},{booth_z}) refused from here, trying next"));
    }
    if !opened {
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
    if lifescape_gamedata::food::best_food(&bank_refs).is_none() {
        keep.extend(carried.iter().filter(|(held, _)| {
            (bot.tables.obj_name)(*held).is_some_and(|name| lifescape_gamedata::food::heal(&name) > 0)
        }).map(|(held, _)| *held));
    }
    keep.sort();
    keep.dedup();
    bot.bank_deposit_all(side_bank, &keep, 25)?;
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
    for slot in [3, 0, 4, 7, 5, 2, 9, 10, 12, 1] {
        for entry in plan.get(&slot).cloned().unwrap_or_default().into_iter().take(2) {
            let Ok(obj) = obj_id(bot, &entry.name) else {
                continue;
            };
            if bot.inventory().contains_key(&obj) || free_slots(bot) <= crate::kit::survival::FOOD_TARGET {
                continue;
            }
            let _ = bot.bank_withdraw(obj, 1, 25);
        }
        if free_slots(bot) <= crate::kit::survival::FOOD_TARGET {
            break;
        }
    }
    crate::kit::survival::withdraw_food(bot)?;
    bot.close_modal()?;
    bot.pump(5)?;
    for (slot, name) in equip_plan(bot, &plan)?.iter() {
        bot.log(&format!("equipped {name} (slot {slot})"));
    }
    Ok(true)
}

/// Re-wear the best kit from what is actually carried (pack + worn):
/// after a death the pack dropped, after a loot the best changed. One
/// evaluation per event, never per kill or per tick; no bank trip and
/// never a buy — the bank only enters at prepare_at_bank on the way in.
/// Logs the slots that changed, quiet when nothing does.
pub fn rewear(bot: &mut Bot, why: &str) -> Result<(), ScriptError> {
    let before: HashMap<i64, u16> = ARMOUR_SLOTS
        .into_iter()
        .chain([WEAPON_SLOT])
        .filter_map(|slot| worn_obj(bot, slot).map(|obj| (slot, obj)))
        .collect();
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
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

#[cfg(test)]
mod tests {
    use super::{loss_cost, prepare_at_bank, WEAPON_SLOT};
    use crate::kit::style::{fight_weapon_style, style_damage_key};
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

    /// No service: the bank walk fails `nav-down`, which `prepare_at_bank`
    /// propagates typed by contract (no walk, no equip-and-continue).
    /// The direct `walk_to` below proves the root cause is `nav-down`
    /// and nothing else (never a local plan).
    #[test]
    fn prepare_offline_falls_back_to_equip() {
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        crate::wire_nav(&mut bot);
        bot.world.level = 0;
        bot.world.x = 3220;
        bot.world.z = 3220;
        bot.world.stats.insert(3, (10, 0));
        let error = prepare_at_bank(&mut bot).expect_err("no service, nav-down is loud");
        assert!(
            matches!(&error, crate::ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
        let error = bot.walk_to(3200, 3200, bot.world.level, 100).expect_err("direct walk is nav-down");
        assert!(
            matches!(&error, lifescape_sdk::Error::Travel(detail) if detail.contains("nav-down")),
            "nav-down and nothing else, got: {error}"
        );
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
            super::gear_score(&bot, &stabby, WEAPON_SLOT, fight_weapon_style, style_damage_key)
                > super::gear_score(&bot, &slashy, WEAPON_SLOT, fight_weapon_style, style_damage_key),
            "controlled trains stab: the stab blade must win"
        );
    }

    /// Equal scores keep the cheaper kit: bronze before steel (dump shop
    /// prices 26 vs 325), unknown names last.
    #[test]
    fn ties_keep_the_cheap_kit() {
        assert!(loss_cost("bronze_sword") < loss_cost("steel_sword"), "bronze cheaper to lose");
        assert_eq!(loss_cost("no_such_item_xyz"), i64::MAX, "unknown prices never win ties");
    }
}
