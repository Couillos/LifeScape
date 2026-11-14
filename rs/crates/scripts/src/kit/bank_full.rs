//! Banque pleine : le dépôt d'un voyage à la banque garde ce que la
//! banque refuse au lieu d'abandonner la tâche.
//!
//! Serveur (`interface_bank/scripts/bank.rs2`, `bank_check_allowed`) : un
//! compte F2P dont la banque occupe déjà `^bank_free_slots` (60) cases ne
//! dépose plus aucun type NOUVEAU (« You need to subscribe for a members
//! account if you want to store any more items in the bank. ») ; un type
//! déjà en banque s'empile toujours ; un objet `unbankable` est refusé
//! (« A magical force prevents you from banking this item! »).
//!
//! Live bst29bh9 2026-11-14 : banque à 60 types, le marteau acheté pour
//! la forge n'y entrait plus ; chaque fonte et chaque meunerie échouait
//! `deposit did not clear` sur ce seul marteau, alors que le sac avait 27
//! cases libres pour la charge. Règle : déposer ce qui rentre, garder ce
//! que la banque refuse (outils compris), n'échouer que quand la charge ne
//! tient plus (sac plein), ou quand un reste n'a aucune raison de rester
//! (type déjà en banque, aucun refus du serveur).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use lifescape_sdk::Bot;

use crate::kit::pack::{free_slots, pack_slots};
use crate::ScriptError;

/// Refus « banque pleine » du serveur (première ligne du message).
pub const BANK_FULL_TEXT: &str = "subscribe for a members account if you want to";
/// Refus d'un objet qu'aucune banque ne prend.
pub const UNBANKABLE_TEXT: &str = "magical force prevents you from banking";

/// True quand un refus de dépôt du serveur est arrivé depuis `seen`.
pub fn deposit_refused_since(bot: &Bot, seen: usize) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|message| {
        let lower = message.to_lowercase();
        lower.contains(BANK_FULL_TEXT) || lower.contains(UNBANKABLE_TEXT)
    })
}

/// True quand la banque pleine a refusé un dépôt depuis `seen`.
pub fn bank_full_since(bot: &Bot, seen: usize) -> bool {
    bot.world.messages.get(seen..).unwrap_or(&[]).iter().any(|message| message.to_lowercase().contains(BANK_FULL_TEXT))
}

/// Verdict pur d'un dépôt inachevé : `leftovers` = types hors `keep`
/// restés dans le sac, `bank` = banque ouverte, `refused` = le serveur a
/// refusé un dépôt, `free` = cases libres du sac. `Ok(types gardés)` quand
/// chaque reste est un type que la banque n'a pas et que le serveur a
/// refusé, et que la charge a encore de la place ; sinon la cause.
pub fn deposit_verdict(leftovers: &[u16], bank: &HashMap<u16, u32>, refused: bool, free: usize) -> Result<Vec<u16>, String> {
    if leftovers.is_empty() {
        return Ok(Vec::new());
    }
    if !refused {
        return Err("deposit did not clear".to_string());
    }
    if let Some(stacked) = leftovers.iter().find(|id| bank.get(id).copied().unwrap_or(0) > 0) {
        return Err(format!("deposit did not clear (obj {stacked} already banked, never refused)"));
    }
    if free == 0 {
        return Err(format!("bank full: types {leftovers:?} refused and the pack has no room left for a load"));
    }
    Ok(leftovers.to_vec())
}

/// Dépose tout sauf `keep` ; les types que la banque refuse restent dans
/// le sac (loggés), l'échec n'arrive que quand la charge ne tient plus.
/// Renvoie les types gardés faute de place en banque.
pub fn deposit_what_fits(bot: &mut Bot, side_com: u16, keep: &[u16]) -> Result<Vec<u16>, ScriptError> {
    let seen = bot.world.messages.len();
    if bot.bank_deposit_all(side_com, keep, 25)? {
        return Ok(Vec::new());
    }
    let mut leftovers: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).filter(|held| !keep.contains(held)).collect();
    leftovers.sort_unstable();
    leftovers.dedup();
    let bank = bot.bank_items();
    match deposit_verdict(&leftovers, &bank, deposit_refused_since(bot, seen), free_slots(bot)) {
        Ok(kept) => {
            if !kept.is_empty() {
                let names: Vec<String> = kept.iter().map(|id| (bot.tables.obj_name)(*id).unwrap_or_else(|| format!("obj{id}"))).collect();
                bot.log(&format!("bank full: kept {names:?} in the pack ({} slots free for the load)", free_slots(bot)));
            }
            Ok(kept)
        }
        Err(cause) => Err(ScriptError::Failed(format!("{cause} at {:?}", bot.position()))),
    }
}

/// The deposit is verified, never assumed: leftovers are a loud error,
/// not a silent carry to the spot.
pub fn require_deposit_cleared(bot: &Bot, deposited: bool) -> Result<(), ScriptError> {
    if deposited {
        return Ok(());
    }
    Err(ScriptError::Failed(format!("deposit did not clear at {:?}", bot.position())))
}

#[cfg(test)]
mod tests {
    use super::{bank_full_since, deposit_refused_since, deposit_verdict};
    use std::collections::HashMap;

    /// Live bst29bh9 : le marteau, type nouveau pour une banque à 60
    /// types, est refusé ; le sac a 27 cases : la fonte continue.
    #[test]
    fn a_refused_new_type_stays_in_the_pack() {
        let bank: HashMap<u16, u32> = [(2351, 4)].into_iter().collect();
        assert_eq!(deposit_verdict(&[2347], &bank, true, 27), Ok(vec![2347]), "hammer kept, load fits");
        assert_eq!(deposit_verdict(&[], &bank, false, 0), Ok(Vec::new()), "nothing left: cleared");
    }

    /// Ce qui n'a aucune raison de rester reste une erreur : aucun refus
    /// du serveur, un type déjà en banque (il s'empile toujours), ou un
    /// sac plein où la charge ne tient plus.
    #[test]
    fn unexplained_leftovers_and_full_packs_still_fail() {
        let bank: HashMap<u16, u32> = [(2351, 4)].into_iter().collect();
        assert!(deposit_verdict(&[2347], &bank, false, 27).is_err(), "no refusal seen");
        assert!(deposit_verdict(&[2351], &bank, true, 27).unwrap_err().contains("already banked"));
        assert!(deposit_verdict(&[2347], &bank, true, 0).unwrap_err().contains("bank full"), "no room for the load");
    }

    /// Les deux refus du serveur se lisent dans le chat, depuis l'index vu.
    #[test]
    fn refusals_read_from_the_chat() {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.messages.push("You need to subscribe for a members account if you want to".to_string());
        assert!(deposit_refused_since(&bot, 0) && bank_full_since(&bot, 0));
        assert!(!deposit_refused_since(&bot, 1), "index tracks seen messages");
        bot.world.messages.push("A magical force prevents you from banking this item!".to_string());
        assert!(deposit_refused_since(&bot, 1) && !bank_full_since(&bot, 1));
    }
}
