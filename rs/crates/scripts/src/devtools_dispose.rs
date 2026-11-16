//! `check_dispose [keep...]`: staff only. Empties a staff account's bank
//! of its test leftovers (live checks stage cheated inputs and keep their
//! products: the F2P bank holds 60 types, `bank.constant`
//! `^bank_free_slots`). AGENTS.md rule 13: staff goods never reach
//! another account, so nothing is sold, listed, traded or dropped where
//! a bot walks; the load is dropped in a members-only spot no F2P bot
//! reaches (Catherby beach, behind members gates) and despawns there.
//! Coins and the `keep` names stay banked.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::devtools::{cheat_teleport, obj_id_exact, require_admin};
use crate::kit::bank::{live_bank_booth, nearest_bank};
use crate::kit::pack::{free_slots, pack_slots};
use crate::{ScriptError, ScriptResult};

/// Members-only drop spot (Catherby beach): no F2P path reaches it.
const DROP_SPOT: (i32, i32, i32) = (2852, 3428, 0);
/// Rounds before giving up (each round: one pack of notes).
const MAX_ROUNDS: u32 = 12;

pub fn dev_check_dispose(bot: &mut Bot, args: &[String]) -> ScriptResult {
    require_admin(bot)?;
    let mut keep: Vec<u16> = vec![obj_id_exact("coins")?];
    for name in args {
        keep.push(obj_id_exact(name)?);
    }
    let home = (bot.world.x, bot.world.z, bot.world.level);
    for round in 0..MAX_ROUNDS {
        // The pack first: a full bank refuses its deposit.
        drop_carried(bot, &keep, home)?;
        let bank = nearest_bank(bot)?;
        bot.approach(bank.x as i32, bank.z as i32, bank.level as i32, 6, 3000)?;
        let Some((booth_x, booth_z, booth)) = live_bank_booth(bot, bank.x as i32, bank.z as i32) else {
            return Err(ScriptError::Failed(format!("no bank booth near ({},{})", bank.x, bank.z)));
        };
        if !bot.open_bank(booth_x, booth_z, booth, 100)? {
            return Err(ScriptError::Failed(format!("bank did not open at ({booth_x},{booth_z})")));
        }
        let banked: Vec<(u16, u32)> = bot.bank_items().into_iter().filter(|(id, count)| *count > 0 && !keep.contains(id)).collect();
        if banked.is_empty() {
            bot.close_modal()?;
            bot.log(&format!("bank holds only the keep list after {round} rounds"));
            return Ok(0);
        }
        for (base, count) in banked {
            if free_slots(bot) == 0 {
                break;
            }
            match crate::kit::spell_load::note_of(base) {
                Some(note) => {
                    bot.bank_withdraw_noted(base, note, count, 40)?;
                }
                None => {
                    bot.bank_withdraw(base, count.min(free_slots(bot) as u32), 40)?;
                }
            }
        }
        bot.close_modal()?;
        bot.log(&format!("round {round}: a pack withdrawn"));
    }
    Err(ScriptError::Failed(format!("bank still holds leftovers after {MAX_ROUNDS} rounds")))
}

/// Teleport to the members-only spot, drop everything carried but the
/// keep list, teleport back to `home`. No-op on a clean pack.
fn drop_carried(bot: &mut Bot, keep: &[u16], home: (i32, i32, i32)) -> Result<(), ScriptError> {
    let carried: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).filter(|held| !keep.contains(held)).collect();
    if carried.is_empty() {
        return Ok(());
    }
    let (drop_x, drop_z, drop_level) = DROP_SPOT;
    if !cheat_teleport(bot, drop_level, drop_x, drop_z, 100)? {
        return Err(ScriptError::Failed(format!("teleport to the drop spot failed from {:?}", bot.position())));
    }
    for held in carried {
        bot.drop(held, 750)?;
    }
    bot.log(&format!("dropped a pack at {DROP_SPOT:?}"));
    if !cheat_teleport(bot, home.2, home.0, home.1, 100)? {
        return Err(ScriptError::Failed(format!("teleport home failed from {:?}", bot.position())));
    }
    Ok(())
}
