//! Gear: best-kit planning, equipping and bank-backed restock. Moved out
//! of the melee script: "s'équiper au mieux" is a graph task
//! (`equip_best`), executed here. Rewear after death or loot never buys
//! and never bank-trips — the bank only enters at `prepare_at_bank` on
//! the way in.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::pack::{free_slots, obj_id};
use crate::ScriptError;

pub use crate::kit::combat_bank::{magic_ammo, prepare_at_bank, prepare_at_bank_for};

pub const ARMOUR_SLOTS: [i64; 10] = [0, 1, 2, 4, 5, 7, 9, 10, 12, 13];
/// Quiver (arrows): worn only by a ranged run (PRD3 lot 6).
pub const AMMO_SLOT: i64 = 13;
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

/// `available_gear` plus the last known bank (open, or closed and kept
/// in `inv_closed`), market locks excluded: what a bank leg or an
/// `equip` step can wear. `inventory()` alone counts the open bank only.
pub fn owned_gear(bot: &Bot) -> HashMap<u16, String> {
    let mut found = available_gear(bot);
    for id in crate::kit::pack::banked_ids(bot) {
        if let Some(name) = (bot.tables.obj_name)(id) {
            found.insert(id, name);
        }
    }
    found
}

/// Worn object per slot right now (weapon and armour slots).
pub fn worn_snapshot(bot: &Bot) -> HashMap<i64, u16> {
    ARMOUR_SLOTS.into_iter().chain([WEAPON_SLOT]).filter_map(|slot| worn_obj(bot, slot).map(|obj| (slot, obj))).collect()
}

/// `equip_plan`, then one log line per slot that changed (`{what}
/// <objet> (slot n)`), quiet when nothing did (live 2026-11-14 : un
/// « equipped bronze_med_helm » par voyage banque). Returns the changed
/// slots with the object now worn (empty name when emptied).
pub fn equip_logged(bot: &mut Bot, plan: &HashMap<i64, Vec<knowledge::equipment::Wearable>>, what: &str) -> Result<Vec<(i64, String)>, ScriptError> {
    let before = worn_snapshot(bot);
    equip_plan(bot, plan)?;
    let after = worn_snapshot(bot);
    let mut changed = Vec::new();
    for slot in changed_slots(&before, &after) {
        let name = after.get(&slot).and_then(|obj| (bot.tables.obj_name)(*obj)).unwrap_or_default();
        bot.log(&format!("{what} {name} (slot {slot})"));
        changed.push((slot, name));
    }
    Ok(changed)
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
        // Arrows ride only a ranged run (a melee bot keeps them banked).
        if entry.wearpos == AMMO_SLOT && bot.combat_style != "ranged" {
            continue;
        }
        // The weapon matches the run: a melee run never plans a bow,
        // crossbow, throwable or staff (will_field live: shortbow wielded,
        // "There is no ammo left in your quiver." on every click).
        if !weapon_fits_style(&entry, &bot.combat_style) {
            continue;
        }
        // Primary slot only: a two-handed weapon also covers the shield
        // slot (wearpos2 5), and listing it there made equip_plan wield
        // the shortbow "as a shield" over the planned pickaxe.
        if entry.wearpos == WEAPON_SLOT || ARMOUR_SLOTS.contains(&entry.wearpos) {
            plan.entry(entry.wearpos).or_default().push(entry.clone());
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
        if *slot == WEAPON_SLOT && bot.combat_style == "melee" {
            // Melee prefers a one-handed weapon so the shield slot stays
            // useful; bows are two-handed by nature, never demoted.
            cands.sort_by_key(|entry| knowledge::equipment::covers_slot(entry, 5));
            // A woodcutting axe or a pickaxe is a tool (live 2026-11-14,
            // pia_brown and leo_king swinging bronze/steel axes with a
            // sword or a mace owned): last resort, only before fists.
            cands.sort_by_key(knowledge::gear::is_tool);
        }
    }
    // Server ammo rule (player_ranged.rs2): an arrow above the bow's tier
    // is refused, so the quiver follows the bow the plan wears.
    if let Some(bow_tier) = plan.get(&WEAPON_SLOT).and_then(|cands| cands.first()).map(|bow| bow.tier.max(1)) {
        if let Some(arrows) = plan.get_mut(&AMMO_SLOT) {
            arrows.retain(|arrow| arrow.tier <= bow_tier);
        }
    }
    plan
}

/// True unless `entry` is a weapon the run's style never fights with
/// (gamedata gear score < 0: no bow or staff in melee, only bows in
/// ranged, only staves in magic). Armour always fits.
pub fn weapon_fits_style(entry: &knowledge::equipment::Wearable, style: &str) -> bool {
    entry.wearpos != WEAPON_SLOT || knowledge::gear::score(entry, knowledge::gear::Style::parse(style)) >= 0
}

/// Name of the wielded weapon when the run must not fight with it: a
/// bow, crossbow, throwable or staff in melee, a melee weapon in ranged
/// (live 2026-11-14, ryan_park « ranged chickens » à la dague de fer).
/// Magic casts whatever is wielded: None.
pub fn off_style_weapon(bot: &Bot) -> Option<String> {
    if bot.combat_style != "melee" && bot.combat_style != "ranged" {
        return None;
    }
    let name = worn_obj(bot, WEAPON_SLOT).and_then(|obj| knowledge::obj_by_id(usize::from(obj)).and_then(|row| row.name))?;
    let entry = knowledge::equipment::by_name(name)?;
    (!weapon_fits_style(&entry, &bot.combat_style)).then(|| name.to_string())
}

/// Take the wielded weapon off (worn tab "Remove": equip.rs2
/// `[inv_button1,wornitems:wear]` -> `~unequip`, refused with "You don't
/// have enough free space to do that." on a full pack). Typed failure
/// when it stays wielded: a run never fights with an off-style weapon.
fn unwield_weapon(bot: &mut Bot, name: &str) -> Result<(), ScriptError> {
    let Some(obj) = worn_obj(bot, WEAPON_SLOT) else { return Ok(()) };
    if free_slots(bot) == 0 {
        return Err(ScriptError::Failed(format!("{} run wields {name} and the pack is full: cannot unwield it", bot.combat_style)));
    }
    let worn_com = (bot.tables.com_id)(WORN_COM_NAME)
        .ok_or_else(|| ScriptError::Usage(format!("{WORN_COM_NAME} missing from component table")))?;
    bot.inv_button(1, obj, WEAPON_SLOT as u8, worn_com)?;
    let target = bot.world.ticks + 10;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if worn_obj(bot, WEAPON_SLOT).is_none() {
            bot.log(&format!("unwielded {name}: no {} weapon wieldable", bot.combat_style));
            return Ok(());
        }
    }
    Err(ScriptError::Failed(format!("{} run still wields {name} after Remove", bot.combat_style)))
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
    // Ranged and magic runs score through the gear resolver (bow, arrow
    // strength, robes): the melee formula below would always pick a
    // sword over a bow (PRD3 lot 6).
    if bot.combat_style != "melee" {
        let style = knowledge::gear::Style::parse(&bot.combat_style);
        return knowledge::gear::score(entry, style) as f64;
    }
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

pub(crate) fn worn_obj(bot: &Bot, slot: i64) -> Option<u16> {
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

/// Wield one carried object by id (arrows merge into the worn quiver
/// stack). True when the pack no longer holds it afterwards.
pub fn wield_held(bot: &mut Bot, obj: u16) -> Result<bool, ScriptError> {
    let wearable = knowledge::obj_by_id(usize::from(obj)).and_then(|entry| entry.name).and_then(knowledge::equipment::by_name);
    if wearable.is_none() {
        return Ok(false);
    }
    let Some(pack_com) = bot.backpack().0 else { return Ok(false) };
    let Some(slot) = bot.backpack().1.into_iter().find(|(_, (held, _))| *held == obj).map(|(slot, _)| slot) else { return Ok(false) };
    bot.op_held(2, obj, slot, pack_com)?;
    let target = bot.world.ticks + 10;
    while bot.world.ticks < target {
        bot.pump(2)?;
        if !bot.backpack().1.iter().any(|(_, (held, _))| *held == obj) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Keep the quiver loaded on a ranged run (PRD3 lot 6): an empty slot 13
/// wields the best carried arrow the worn bow accepts (server ammo rule:
/// arrow tier <= bow tier). True when arrows are worn afterwards.
pub fn refill_quiver(bot: &mut Bot) -> Result<bool, ScriptError> {
    if worn_obj(bot, AMMO_SLOT).is_some() {
        return Ok(true);
    }
    let bow_tier = worn_obj(bot, WEAPON_SLOT)
        .and_then(|id| knowledge::obj_by_id(usize::from(id)).and_then(|obj| obj.name))
        .and_then(knowledge::equipment::by_name)
        .map(|bow| bow.tier.max(1))
        .unwrap_or(1);
    let mut arrows: Vec<knowledge::equipment::Wearable> = bot
        .inventory()
        .keys()
        .filter_map(|id| knowledge::obj_by_id(usize::from(*id)).and_then(|obj| obj.name))
        .filter_map(knowledge::equipment::by_name)
        .filter(|entry| entry.wearpos == AMMO_SLOT && entry.rangebonus > 0 && entry.tier <= bow_tier)
        .collect();
    arrows.sort_by_key(|entry| std::cmp::Reverse(entry.rangebonus));
    for entry in arrows {
        if equip(bot, &entry)? {
            bot.log(&format!("ok quiver refilled with {}", entry.name));
            return Ok(true);
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
    // No melee weapon owned (or none wearable): a leftover bow or staff
    // comes off, bare fists beat a refused click.
    if let Some(name) = off_style_weapon(bot) {
        unwield_weapon(bot, &name)?;
    }
    Ok(equipped)
}

/// Emplacements dont l'objet porté a changé entre deux relevés.
pub fn changed_slots(before: &HashMap<i64, u16>, after: &HashMap<i64, u16>) -> Vec<i64> {
    let mut slots: Vec<i64> = before.keys().chain(after.keys()).copied().filter(|slot| before.get(slot) != after.get(slot)).collect();
    slots.sort_unstable();
    slots.dedup();
    slots
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

/// Re-wear the best kit from what is actually carried (pack + worn):
/// after a death the pack dropped, after a loot the best changed. One
/// evaluation per event, never per kill or per tick; no bank trip and
/// never a buy — the bank only enters at prepare_at_bank on the way in.
/// Logs the slots that changed, quiet when nothing does.
pub fn rewear(bot: &mut Bot, why: &str) -> Result<(), ScriptError> {
    let available = available_gear(bot);
    let plan = gear_plan(bot, &available, crate::kit::style::fight_weapon_style, crate::kit::style::style_damage_key);
    equip_logged(bot, &plan, &format!("{why}: now wearing"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The bank leg of a magic run withdraws the ORDERED spell's runes
    /// (fire strike: mind, fire, air), not the wind-strike pair: a
    /// `fight:magic/fire_strike` run left the fire runes banked.
    #[test]
    fn magic_run_withdraws_the_ordered_spell_runes() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        bot.combat_style = "magic".to_string();
        assert_eq!(super::magic_ammo(&bot), vec!["mindrune".to_string(), "airrune".to_string()], "no spell: wind strike");
        bot.combat_spell = Some("fire_strike".to_string());
        assert_eq!(super::magic_ammo(&bot), vec!["mindrune".to_string(), "firerune".to_string(), "airrune".to_string()]);
    }

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
            quest: None,
            tier: -1,
            rangebonus: -1,
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

    fn plan_names(plan: &std::collections::HashMap<i64, Vec<lifescape_gamedata::equipment::Wearable>>, slot: i64) -> Vec<String> {
        plan.get(&slot).map(|cands| cands.iter().map(|entry| entry.name.clone()).collect()).unwrap_or_default()
    }

    /// will_field live: melee run owning a shortbow and a bronze pickaxe
    /// logged "equipped bronze_pickaxe (slot 3)" then "equipped shortbow
    /// (slot 5)": the two-handed bow sat in the shield candidates and was
    /// wielded over the pickaxe. Melee plans the pickaxe only, never the
    /// bow in any slot; alone, the bow plans nothing; ranged plans the bow.
    #[test]
    fn melee_plan_never_holds_a_bow() {
        let mut bot = stat_bot(1, 1, 1);
        bot.combat_style = "melee".to_string();
        let bow = crate::kit::pack::obj_id(&bot, "shortbow").expect("shortbow id");
        let pickaxe = crate::kit::pack::obj_id(&bot, "bronze_pickaxe").expect("pickaxe id");
        let both = std::collections::HashMap::from([(bow, "Shortbow".to_string()), (pickaxe, "Bronze pickaxe".to_string())]);
        let plan = super::gear_plan(&bot, &both, fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT), vec!["bronze_pickaxe".to_string()]);
        assert!(plan.values().flatten().all(|entry| entry.name != "shortbow"), "no slot holds the bow: {plan:?}");
        let bow_only = std::collections::HashMap::from([(bow, "Shortbow".to_string())]);
        assert!(super::gear_plan(&bot, &bow_only, fight_weapon_style, style_damage_key).is_empty(), "bow alone: unarmed");
        bot.combat_style = "ranged".to_string();
        let plan = super::gear_plan(&bot, &both, fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT), vec!["shortbow".to_string()], "ranged wields the bow only");
        assert!(plan_names(&plan, 5).is_empty(), "the bow is no shield candidate");
    }

    /// A wielded bow is off-style for melee only (the unwield trigger).
    #[test]
    fn wielded_bow_is_off_style_in_melee() {
        let mut bot = stat_bot(1, 1, 1);
        let bow = crate::kit::pack::obj_id(&bot, "shortbow").expect("shortbow id");
        let worn_com = (bot.tables.com_id)(super::WORN_COM_NAME).expect("worn com");
        bot.world.inv.insert(worn_com, std::collections::HashMap::from([(WEAPON_SLOT as u8, (bow, 1))]));
        bot.combat_style = "melee".to_string();
        assert_eq!(super::off_style_weapon(&bot).as_deref(), Some("shortbow"));
        bot.combat_style = "ranged".to_string();
        assert_eq!(super::off_style_weapon(&bot), None);
        bot.combat_style = "melee".to_string();
        let pickaxe = crate::kit::pack::obj_id(&bot, "bronze_pickaxe").expect("pickaxe id");
        bot.world.inv.insert(worn_com, std::collections::HashMap::from([(WEAPON_SLOT as u8, (pickaxe, 1))]));
        assert_eq!(super::off_style_weapon(&bot), None, "a pickaxe swings");
    }

    fn owned(bot: &Bot, names: &[&str]) -> std::collections::HashMap<u16, String> {
        names.iter().map(|name| (crate::kit::pack::obj_id(bot, name).expect("obj id"), name.to_string())).collect()
    }

    /// Live 2026-11-14 (pia_brown, leo_king) : la hache de bûcheron est
    /// un outil, jamais l'arme de combat quand une vraie arme est
    /// possédée (masse, épée, dague) ; seule, elle reste mieux que les
    /// poings.
    #[test]
    fn melee_plan_demotes_woodcutting_axes() {
        let mut bot = stat_bot(54, 54, 54);
        bot.combat_style = "melee".to_string();
        let plan = super::gear_plan(&bot, &owned(&bot, &["steel_axe", "iron_mace", "iron_dagger"]), fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT).first().map(String::as_str), Some("iron_mace"), "{:?}", plan_names(&plan, WEAPON_SLOT));
        assert_eq!(plan_names(&plan, WEAPON_SLOT).last().map(String::as_str), Some("steel_axe"), "the axe is the last resort");
        let plan = super::gear_plan(&bot, &owned(&bot, &["bronze_axe", "bronze_sword", "iron_dagger"]), fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT).first().map(String::as_str), Some("bronze_sword"));
        let plan = super::gear_plan(&bot, &owned(&bot, &["steel_axe"]), fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT), vec!["steel_axe".to_string()], "alone, the axe beats fists");
        let plan = super::gear_plan(&bot, &owned(&bot, &["black_battleaxe", "iron_dagger"]), fight_weapon_style, style_damage_key);
        assert_eq!(plan_names(&plan, WEAPON_SLOT).first().map(String::as_str), Some("black_battleaxe"), "a battleaxe is a weapon");
    }

    /// Live 2026-11-14 (ryan_park) : une course à distance ne garde jamais
    /// une dague en main ; l'arc et l'armure de cuir passent devant.
    #[test]
    fn ranged_plan_wields_the_bow_never_the_dagger() {
        let mut bot = stat_bot(49, 49, 49);
        bot.world.stats.insert(4, (1, 0));
        bot.combat_style = "ranged".to_string();
        let plan = super::gear_plan(
            &bot,
            &owned(&bot, &["iron_dagger", "shortbow", "longbow", "leather_armour", "bronze_med_helm", "iron_arrow", "bronze_arrow"]),
            fight_weapon_style,
            style_damage_key,
        );
        assert_eq!(plan_names(&plan, WEAPON_SLOT).first().map(String::as_str), Some("shortbow"));
        assert!(!plan_names(&plan, WEAPON_SLOT).contains(&"iron_dagger".to_string()));
        assert_eq!(plan_names(&plan, 4).first().map(String::as_str), Some("leather_armour"));
        let mut arrows = plan_names(&plan, super::AMMO_SLOT);
        arrows.sort();
        assert_eq!(arrows, vec!["bronze_arrow".to_string(), "iron_arrow".to_string()], "the shortbow takes both");
        let worn_com = (bot.tables.com_id)(super::WORN_COM_NAME).expect("worn com");
        let dagger = crate::kit::pack::obj_id(&bot, "iron_dagger").expect("dagger");
        bot.world.inv.insert(worn_com, std::collections::HashMap::from([(WEAPON_SLOT as u8, (dagger, 1))]));
        assert_eq!(super::off_style_weapon(&bot).as_deref(), Some("iron_dagger"), "ranged never swings a dagger");
    }

    /// Live 2026-11-14 : « equipped bronze_med_helm » à chaque voyage
    /// banque. Seuls les emplacements qui changent se loggent.
    #[test]
    fn unchanged_slots_stay_quiet() {
        let before = std::collections::HashMap::from([(0i64, 1139u16), (3, 1203)]);
        assert!(super::changed_slots(&before, &before.clone()).is_empty());
        let after = std::collections::HashMap::from([(0i64, 1139u16), (3, 841)]);
        assert_eq!(super::changed_slots(&before, &after), vec![3]);
    }

    /// Equal scores keep the cheaper kit: bronze before steel (dump shop
    /// prices 26 vs 325), unknown names last.
    #[test]
    fn ties_keep_the_cheap_kit() {
        assert!(loss_cost("bronze_sword") < loss_cost("steel_sword"), "bronze cheaper to lose");
        assert_eq!(loss_cost("no_such_item_xyz"), i64::MAX, "unknown prices never win ties");
    }
}
