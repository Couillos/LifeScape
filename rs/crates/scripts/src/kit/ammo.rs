//! Ammunition: the weapon refusals of an attack click and what the run
//! does about them. Server source (read-only):
//! content/scripts/skill_combat/scripts/player/player_ranged.rs2
//! `player_ranged_check_ammo` — an empty quiver says "There is no ammo
//! left in your quiver.", a mismatched one "You can't use that ammo with
//! your bow." / "Your bow isn't powerful enough for those arrows.". The
//! click is refused every time: re-clicking is a loop (will_field live,
//! melee run with a shortbow, minutes of op_npc2 + "no ammo").

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_gamedata as knowledge;
use lifescape_sdk::Bot;

use crate::kit::gear::{refill_quiver, rewear, AMMO_SLOT, WEAPON_SLOT, WORN_COM_NAME};
use crate::ScriptError;

/// Cheapest arrow every bow accepts (tier 1): what a blocked ranged run
/// asks the brain to obtain.
pub const BASE_ARROW: &str = "bronze_arrow";

/// A refused attack click caused by the wielded weapon or its ammo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponRefusal {
    /// Ranged weapon wielded, quiver empty.
    NoAmmo,
    /// Quiver holds ammo the wielded weapon refuses (kind or tier).
    WrongAmmo,
}

/// Classify one game message; None for anything else.
pub fn weapon_refusal(line: &str) -> Option<WeaponRefusal> {
    let lowered = line.to_lowercase();
    if lowered.contains("no ammo left in your quiver") {
        Some(WeaponRefusal::NoAmmo)
    } else if lowered.contains("can't use that ammo with your") || lowered.contains("isn't powerful enough for those arrows") {
        Some(WeaponRefusal::WrongAmmo)
    } else {
        None
    }
}

/// What the run does about a weapon refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmmoRemedy {
    /// Melee or magic run: the wrong weapon is wielded, re-plan the kit.
    Rewear,
    /// Ranged run, quiver loaded again: keep hunting.
    Resume,
    /// Ranged run, quiver empty, the bank holds arrows the bow takes.
    BankTrip,
    /// Ranged run, no arrows anywhere: blocked typed, the brain obtains.
    Blocked,
}

/// Pure decision. `quiver_loaded`: the ranged quiver holds usable ammo
/// after the pack refill and the ground pickup.
pub fn ammo_remedy(style: &str, quiver_loaded: bool, bank_has_ammo: bool, no_bank: bool) -> AmmoRemedy {
    if style != "ranged" {
        AmmoRemedy::Rewear
    } else if quiver_loaded {
        AmmoRemedy::Resume
    } else if bank_has_ammo && !no_bank {
        AmmoRemedy::BankTrip
    } else {
        AmmoRemedy::Blocked
    }
}

/// Outcome the fight loop acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmmoFix {
    Resume,
    BankTrip,
}

fn wearable(id: u16) -> Option<knowledge::equipment::Wearable> {
    knowledge::obj_by_id(usize::from(id)).and_then(|obj| obj.name).and_then(knowledge::equipment::by_name)
}

fn worn(bot: &Bot, slot: i64) -> Option<u16> {
    let worn_com = (bot.tables.com_id)(WORN_COM_NAME)?;
    bot.world.inv.get(&worn_com)?.get(&(slot as u8)).map(|(held, _)| *held)
}

/// Tier of the wielded bow (1 when unknown: bronze always fits).
fn bow_tier(bot: &Bot) -> i64 {
    worn(bot, WEAPON_SLOT).and_then(wearable).map(|bow| bow.tier.max(1)).unwrap_or(1)
}

/// The quiver holds arrows the wielded bow accepts (server ammo rule:
/// arrow tier <= bow tier).
fn quiver_fits_bow(bot: &Bot) -> bool {
    let tier = bow_tier(bot);
    worn(bot, AMMO_SLOT).and_then(wearable).is_some_and(|arrow| arrow.rangebonus > 0 && arrow.tier <= tier)
}

/// Last known bank contents: open now, else the copy kept at close.
fn known_bank(bot: &Bot) -> HashMap<u16, u32> {
    let open = bot.bank_items();
    if !open.is_empty() {
        return open;
    }
    lifescape_sdk::snapshot::bank_items(&bot.world.inv_closed, (bot.tables.com_id)("bank_main:inv"))
}

/// The bank holds arrows the wielded bow accepts.
fn bank_has_ammo(bot: &Bot) -> bool {
    let tier = bow_tier(bot);
    known_bank(bot).into_iter().any(|(id, count)| {
        count > 0 && wearable(id).is_some_and(|arrow| arrow.wearpos == AMMO_SLOT && arrow.rangebonus > 0 && arrow.tier <= tier)
    })
}

/// Ranged run: load the quiver from the pack, then from the arrows on
/// the ground; else a bank trip when the bank stocks arrows the bow
/// takes, else blocked typed on the base arrow.
pub fn ensure_ranged_ammo(bot: &mut Bot, no_bank: bool) -> Result<AmmoFix, ScriptError> {
    let mut loaded = refill_quiver(bot)? && quiver_fits_bow(bot);
    if !loaded {
        crate::kit::loot::recover_arrows(bot)?;
        loaded = refill_quiver(bot)? && quiver_fits_bow(bot);
    }
    match ammo_remedy("ranged", loaded, bank_has_ammo(bot), no_bank) {
        AmmoRemedy::Resume | AmmoRemedy::Rewear => Ok(AmmoFix::Resume),
        AmmoRemedy::BankTrip => {
            bot.log("quiver empty, arrows banked: bank trip");
            Ok(AmmoFix::BankTrip)
        }
        AmmoRemedy::Blocked => {
            bot.log("out of arrows");
            Err(ScriptError::Blocked { missing: vec![BASE_ARROW.to_string()] })
        }
    }
}

/// Act on a weapon refusal. Melee/magic: re-plan the kit (the plan never
/// holds a bow for melee, a leftover one comes off); still wrong after
/// that is a typed failure, never another click. Ranged: a mismatched
/// quiver is re-planned first (arrows follow the bow tier), then the
/// quiver is loaded or the run banks/blocks.
pub fn resolve(bot: &mut Bot, refusal: WeaponRefusal, no_bank: bool) -> Result<AmmoFix, ScriptError> {
    bot.log(&format!("weapon refusal {refusal:?} in a {} run", bot.combat_style));
    if ammo_remedy(&bot.combat_style, false, false, no_bank) == AmmoRemedy::Rewear {
        rewear(bot, "weapon refusal")?;
        if let Some(name) = crate::kit::gear::off_style_weapon(bot) {
            return Err(ScriptError::Failed(format!("{} run still wields {name} after re-plan", bot.combat_style)));
        }
        return Ok(AmmoFix::Resume);
    }
    if refusal == WeaponRefusal::WrongAmmo {
        rewear(bot, "ammo mismatch")?;
    }
    ensure_ranged_ammo(bot, no_bank)
}

#[cfg(test)]
mod tests {
    use super::{ammo_remedy, weapon_refusal, AmmoRemedy, WeaponRefusal};

    /// Exact server lines (player_ranged.rs2): the quiver line with and
    /// without its period (ogre bow), the two mismatch lines; combat
    /// refusals stay with the foe vocabulary.
    #[test]
    fn weapon_refusals_are_classified() {
        assert_eq!(weapon_refusal("There is no ammo left in your quiver."), Some(WeaponRefusal::NoAmmo));
        assert_eq!(weapon_refusal("There is no ammo left in your quiver"), Some(WeaponRefusal::NoAmmo));
        assert_eq!(weapon_refusal("You can't use that ammo with your bow."), Some(WeaponRefusal::WrongAmmo));
        assert_eq!(weapon_refusal("You can't use that ammo with your crossbow."), Some(WeaponRefusal::WrongAmmo));
        assert_eq!(weapon_refusal("Your bow isn't powerful enough for those arrows."), Some(WeaponRefusal::WrongAmmo));
        assert_eq!(weapon_refusal("Someone else is fighting that."), None);
        assert_eq!(weapon_refusal("I can't reach that!"), None);
    }

    /// will_field live: a melee run re-plans its weapon, never re-clicks;
    /// a ranged run reloads, banks, or blocks typed — never loops.
    #[test]
    fn remedy_follows_the_run_style() {
        assert_eq!(ammo_remedy("melee", false, true, false), AmmoRemedy::Rewear);
        assert_eq!(ammo_remedy("magic", false, false, false), AmmoRemedy::Rewear);
        assert_eq!(ammo_remedy("ranged", true, false, false), AmmoRemedy::Resume);
        assert_eq!(ammo_remedy("ranged", false, true, false), AmmoRemedy::BankTrip);
        assert_eq!(ammo_remedy("ranged", false, true, true), AmmoRemedy::Blocked, "--no-bank never banks");
        assert_eq!(ammo_remedy("ranged", false, false, false), AmmoRemedy::Blocked);
    }
}
