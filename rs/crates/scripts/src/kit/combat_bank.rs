//! Voyage banque d'un combat : déposer le butin, garder exactement le
//! kit du style, retirer les pièces prévues, s'équiper, renvoyer les
//! pièces remplacées. Sorti de `gear.rs` (2026-11-14).
//!
//! Live 2026-11-14 (frank_lane, pia_brown, leo_king, bbkjz2vk,
//! bd23dqpq) : la liste gardée au dépôt valait « tout objet portable
//! porté au sac » lue sur `inventory()` (banque ouverte = banque + miroir
//! du sac) ; 20 casques médiums, 5 dagues de fer et 140 flèches de
//! bronze ramassés en butin restaient au sac, chaque kill finissait
//! « pack full » et repartait à la banque, des centaines de trajets sans
//! erreur. Le même `inventory()` faisait croire « déjà porté » un arc ou
//! une épée rangés en banque : jamais retirés (ryan_park à la dague en
//! course à distance, pia_brown à la hache).
//!
//! Règle : garde = munitions du style (flèches que l'arc prévu accepte,
//! runes du sort ordonné) + objets de la chasse (`keep_items` : clé,
//! péage) + nourriture seulement quand la banque n'en a pas. Tout le
//! reste part en banque (butin, doublons, pièces remplacées : la passe de
//! vente normale s'en charge). Les pièces prévues absentes du corps
//! sortent à UNE unité chacune, la nourriture au plan (jamais au point de
//! remplir le sac), puis équipement ; une pièce remplacée retombée au sac
//! repart en banque dans la même visite. Un voyage qui laisse le sac trop
//! plein pour chasser est une erreur typée, jamais une boucle.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::bank::{heal_at_bank, live_bank_booths, nearest_bank};
use crate::kit::gear::{equip_logged, gear_plan, owned_gear, worn_obj, AMMO_SLOT, WEAPON_SLOT};
use crate::kit::pack::{free_slots, obj_id, pack_slots, pack_total};
use crate::kit::style::{fight_weapon_style, style_damage_key};
use crate::skill_walk::walk_checked_at;
use crate::ScriptError;

/// Plan d'équipement : candidats par emplacement, meilleur d'abord.
pub type GearPlan = HashMap<i64, Vec<knowledge::equipment::Wearable>>;

/// Cases libres sous lesquelles `fight` repart aussitôt à la banque
/// (`free_slots <= 1`) : un voyage qui finit là boucle.
pub const HUNT_MIN_FREE: usize = 2;

/// Ordre de retrait des pièces (arme d'abord, comme le brain).
const WITHDRAW_ORDER: [i64; 10] = [WEAPON_SLOT, 4, 7, 0, 5, 10, 9, 2, 12, 1];

fn is_food(bot: &Bot, obj: u16) -> bool {
    (bot.tables.obj_name)(obj).is_some_and(|name| knowledge::food::heal(&name) > 0)
}

fn is_wearable(obj: u16) -> bool {
    knowledge::obj_by_id(usize::from(obj)).and_then(|row| row.name).and_then(knowledge::equipment::by_name).is_some()
}

/// Pièce prévue par emplacement (le premier candidat), arme d'abord ;
/// pas de bouclier sous une arme à deux mains, carquois à part.
pub fn planned_kit(plan: &GearPlan) -> Vec<(i64, String)> {
    let weapon = plan.get(&WEAPON_SLOT).and_then(|cands| cands.first());
    let two_handed = weapon.is_some_and(|entry| knowledge::equipment::covers_slot(entry, 5));
    WITHDRAW_ORDER
        .iter()
        .filter(|slot| !(two_handed && **slot == 5))
        .filter_map(|slot| plan.get(slot).and_then(|cands| cands.first()).map(|entry| (*slot, entry.name.clone())))
        .collect()
}

/// Pièces prévues que ni le corps ni le sac ne portent : une unité de
/// chacune à retirer.
pub fn kit_to_withdraw(bot: &Bot, plan: &GearPlan) -> Vec<u16> {
    planned_kit(plan)
        .into_iter()
        .filter_map(|(slot, name)| obj_id(bot, &name).ok().map(|obj| (slot, obj)))
        .filter(|(slot, obj)| worn_obj(bot, *slot) != Some(*obj) && pack_total(bot, *obj) == 0)
        .map(|(_, obj)| obj)
        .collect()
}

/// Munitions du style : toutes les flèches que l'arc prévu accepte
/// (distance), les runes du sort ordonné (magie), rien en mêlée.
pub fn style_ammo_ids(bot: &Bot, plan: &GearPlan) -> Vec<u16> {
    match bot.combat_style.as_str() {
        "ranged" => plan.get(&AMMO_SLOT).map(|cands| cands.iter().filter_map(|arrow| obj_id(bot, &arrow.name).ok()).collect()).unwrap_or_default(),
        "magic" => magic_ammo(bot).iter().filter_map(|name| obj_id(bot, name).ok()).collect(),
        _ => Vec::new(),
    }
}

/// Runes a magic run's bank leg withdraws (whole stacks): the ordered
/// spell's (`--spell`), wind strike's without one.
pub fn magic_ammo(bot: &Bot) -> Vec<String> {
    let spell = bot.combat_spell.as_deref().unwrap_or(crate::activities::magic_style::DAMAGE_SPELLS[0].0);
    crate::activities::magic_style::spell_runes(spell).into_iter().map(|(name, _)| name).collect()
}

/// Liste gardée au dépôt (module doc) : munitions du style, objets de la
/// chasse, nourriture portée quand la banque n'en a aucune. Jamais une
/// pièce d'équipement : les pièces prévues ressortent à l'unité.
pub fn combat_keep_ids(bot: &Bot, plan: &GearPlan, keep_items: &[(String, u32)], bank_has_food: bool) -> Vec<u16> {
    let mut keep = style_ammo_ids(bot, plan);
    keep.extend(keep_items.iter().filter_map(|(name, _)| obj_id(bot, name).ok()));
    if !bank_has_food {
        keep.extend(pack_slots(bot).into_iter().map(|(_, (held, _))| held).filter(|held| is_food(bot, *held)));
    }
    keep.sort_unstable();
    keep.dedup();
    keep
}

/// Surplus resté au sac après l'équipement : les pièces portables hors
/// de la garde (pièces remplacées, doublons).
pub fn pack_surplus(bot: &Bot, keep: &[u16]) -> Vec<u16> {
    let mut surplus: Vec<u16> = pack_slots(bot).into_iter().map(|(_, (held, _))| held).filter(|held| !keep.contains(held) && is_wearable(*held)).collect();
    surplus.sort_unstable();
    surplus.dedup();
    surplus
}

/// Nourriture à viser : le plan, sans jamais descendre sous
/// `HUNT_MIN_FREE` cases libres (une case par unité).
pub fn food_load(food_target: u32, carried_food: u32, free: usize) -> u32 {
    let room = free.saturating_sub(HUNT_MIN_FREE) as u32;
    food_target.min(carried_food + room)
}

/// Verdict pur de fin de voyage : un sac sous `HUNT_MIN_FREE` cases
/// libres renverrait la chasse à la banque au premier kill.
pub fn trip_verdict(free: usize, pack_labels: &[String]) -> Result<(), ScriptError> {
    if free >= HUNT_MIN_FREE {
        return Ok(());
    }
    Err(ScriptError::Failed(format!("combat bank trip left the pack too full to hunt ({free} free): {}", pack_labels.join(", "))))
}

fn pack_labels(bot: &Bot) -> Vec<String> {
    let mut counts: Vec<(String, u32)> = Vec::new();
    for (_, (held, count)) in pack_slots(bot) {
        let label = (bot.tables.obj_name)(held).unwrap_or_else(|| format!("obj{held}"));
        match counts.iter_mut().find(|(name, _)| *name == label) {
            Some(entry) => entry.1 += count,
            None => counts.push((label, count)),
        }
    }
    counts.into_iter().map(|(name, count)| format!("{count}x {name}")).collect()
}

fn equip_owned(bot: &mut Bot) -> Result<(), ScriptError> {
    let plan = gear_plan(bot, &owned_gear(bot), fight_weapon_style, style_damage_key);
    equip_logged(bot, &plan, "equipped")?;
    Ok(())
}

fn nav_down(error: &ScriptError) -> bool {
    matches!(error, ScriptError::Sdk(lifescape_sdk::Error::Travel(detail)) if detail.contains("nav-down"))
}

/// Ouvre la banque depuis la case de banque : booth le plus proche
/// d'abord, chaque refus passe au suivant. Faux quand aucun n'ouvre.
fn open_booth(bot: &mut Bot, bank_x: i32, bank_z: i32) -> Result<bool, ScriptError> {
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
    let bank_main = (bot.tables.com_id)("bank_main:inv").ok_or_else(|| ScriptError::Usage("bank_main:inv missing from component table".to_string()))?;
    let target = bot.world.ticks + 50;
    while bot.world.ticks < target && !bot.world.inv.contains_key(&bank_main) {
        bot.pump(5)?;
    }
    if !bot.world.inv.contains_key(&bank_main) {
        bot.close_modal()?;
        return Ok(false);
    }
    Ok(true)
}

/// Deposit loot, withdraw the best gear and food for the current levels.
pub fn prepare_at_bank(bot: &mut Bot) -> Result<bool, ScriptError> {
    prepare_at_bank_for(bot, crate::kit::survival::FOOD_TARGET as u32, &[])
}

/// `prepare_at_bank` with the hunt's food load (`food_target` units)
/// and items that ride every trip (`keep_items` as (name, units): the
/// brass key of the Edgeville dungeon giants, the toll coins), kept
/// through the deposit and topped up from the bank. Module doc : garde,
/// retraits, équipement, verdict.
pub fn prepare_at_bank_for(bot: &mut Bot, food_target: u32, keep_items: &[(String, u32)]) -> Result<bool, ScriptError> {
    let bank = match nearest_bank(bot) {
        // No service means no fence line and no walk: typed `nav-down`
        // stays loud instead of degrading into an equip-and-continue.
        Err(error) if nav_down(&error) => return Err(error),
        Ok(bank) => bank,
        Err(_) => {
            equip_owned(bot)?;
            return Ok(false);
        }
    };
    let (bank_x, bank_z) = (bank.x as i32, bank.z as i32);
    bot.intend(&format!("banking at ({bank_x},{bank_z})"));
    match walk_checked_at(bot, bank_x, bank_z, bank.level as i32, 15000) {
        Ok(true) => {}
        Err(error) if nav_down(&error) || crate::kit::survival::is_walk_abort(&error) => return Err(error),
        Ok(false) | Err(_) => {
            equip_owned(bot)?;
            return Ok(false);
        }
    }
    // The bank tile is safe: heal low HP here before the booth.
    heal_at_bank(bot)?;
    if !open_booth(bot, bank_x, bank_z)? {
        return Ok(false);
    }
    let side_bank = (bot.tables.com_id)("bank_side:inv").ok_or_else(|| ScriptError::Usage("bank_side:inv missing from component table".to_string()))?;
    let plan = gear_plan(bot, &owned_gear(bot), fight_weapon_style, style_damage_key);
    let keep = combat_keep_ids(bot, &plan, keep_items, crate::kit::survival::bank_stocks_food(bot));
    crate::kit::bank_full::deposit_what_fits(bot, side_bank, &keep)?;
    // A paid run (`toll`) leaves the bank with the fare in the pack.
    let coins = obj_id(bot, "coins").ok();
    if let Some(coins) = coins {
        let carried = pack_total(bot, coins);
        let short = crate::kit::toll::restock_reserve(bot, (bank_x, bank_z, bank.level as i32), carried)?;
        if short > 0 && bot.bank_items().contains_key(&coins) {
            let _ = bot.bank_withdraw(coins, short, 25);
        }
    }
    for (name, units) in keep_items {
        let Ok(obj) = obj_id(bot, name) else { continue };
        let carried = pack_total(bot, obj);
        let short = units.saturating_sub(carried);
        if short > 0 && bot.bank_items().contains_key(&obj) && (carried > 0 || free_slots(bot) > 0) {
            let _ = bot.bank_withdraw(obj, short, 25);
        }
    }
    // One unit of each planned piece the body does not wear yet.
    for obj in kit_to_withdraw(bot, &plan) {
        if free_slots(bot) > HUNT_MIN_FREE && bot.bank_items().contains_key(&obj) {
            let _ = bot.bank_withdraw(obj, 1, 25);
        }
    }
    // Ammunition rides whole stacks: every arrow the bow takes, the
    // ordered spell's runes.
    for obj in style_ammo_ids(bot, &plan) {
        let stock = bot.bank_items().get(&obj).copied().unwrap_or(0);
        if stock > 0 && (pack_total(bot, obj) > 0 || free_slots(bot) > HUNT_MIN_FREE) {
            let _ = bot.bank_withdraw(obj, stock, 25);
        }
    }
    let load = food_load(food_target, crate::kit::survival::food_count(bot), free_slots(bot));
    crate::kit::survival::withdraw_food_to(bot, load)?;
    bot.close_modal()?;
    bot.pump(5)?;
    equip_logged(bot, &plan, "equipped")?;
    // A replaced piece fell into the pack: back to the bank, same visit.
    let mut after_keep = keep.clone();
    after_keep.extend(coins);
    let surplus = pack_surplus(bot, &after_keep);
    if !surplus.is_empty() && open_booth(bot, bank_x, bank_z)? {
        let names: Vec<String> = surplus.iter().map(|id| crate::kit::pack::obj_label(bot, *id)).collect();
        bot.log(&format!("depositing replaced gear {names:?}"));
        let mut kept: Vec<u16> = pack_slots(bot).into_iter().map(|(_, (held, _))| held).filter(|held| !surplus.contains(held)).collect();
        kept.sort_unstable();
        kept.dedup();
        crate::kit::bank_full::deposit_what_fits(bot, side_bank, &kept)?;
        bot.close_modal()?;
        bot.pump(5)?;
    }
    trip_verdict(free_slots(bot), &pack_labels(bot))?;
    Ok(true)
}

#[cfg(test)]
#[path = "combat_bank_tests.rs"]
mod tests;
