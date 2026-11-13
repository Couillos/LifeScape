//! Toll the brain budgeted (`toll` flag, user decision 2026-11-13): the run
//! may cross the Al Kharid border gate, and the guard takes the fare from
//! the INVENTORY (`border_gate.rs2`: `inv_total(inv, coins) < 10` ⇒ "Oh
//! dear, I don't actually seem to have enough money"). A bot whose coins
//! sit in the bank fetches the fare before the first crossing; bank trips
//! of a paid run keep it (`kit::bank::bank_trip`).

#![forbid(unsafe_code)]

use lifescape_gamedata::toll::FARE;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_slots, pack_total};
use crate::ScriptError;

/// Coins a paid run keeps in the pack.
pub fn fare() -> u32 {
    FARE as u32
}

/// Coins to withdraw so the pack carries the fare (0 when it does).
pub fn fare_short(carried: u32) -> u32 {
    fare().saturating_sub(carried)
}

/// Opt the run into paid crossings (`toll`) and carry the fare. Unpaid
/// runs are untouched.
pub fn arm(bot: &mut Bot, allow_paid: bool) -> Result<(), ScriptError> {
    if !allow_paid {
        return Ok(());
    }
    // The fare first, from a bank on this side (the pick is fenced while
    // paid paths are still off), then the gate opens to the run.
    carry_fare(bot)?;
    bot.allow_paid = true;
    bot.log(&format!("toll gates allowed for this run ({} coins carried)", carried_coins(bot)));
    Ok(())
}

fn carried_coins(bot: &Bot) -> u32 {
    obj_id(bot, "coins").map(|coins| pack_total(bot, coins)).unwrap_or(0)
}

/// Fare in the pack: a bank trip withdraws the shortfall, the whole pack
/// kept. Still short after it (bank empty): typed `Blocked` on the coins
/// the brain files as a `have_coins` dependency.
pub fn carry_fare(bot: &mut Bot) -> Result<(), ScriptError> {
    crate::kit::survival::wait_pack_streamed(bot)?;
    let short = fare_short(carried_coins(bot));
    if short == 0 {
        return Ok(());
    }
    let keep: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    let bank = crate::kit::bank::nearest_bank(bot)?;
    bot.log(&format!("toll: fetching {short} coins for the gate from the bank at ({},{})", bank.x, bank.z));
    crate::kit::bank::bank_trip(bot, &bank, &keep, short)?;
    if fare_short(carried_coins(bot)) > 0 {
        return Err(ScriptError::Blocked { missing: vec![format!("coins:{FARE}")] });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{fare, fare_short};

    #[test]
    fn the_fare_is_ten_coins_carried() {
        assert_eq!(fare(), 10);
        assert_eq!(fare_short(0), 10);
        assert_eq!(fare_short(4), 6);
        assert_eq!(fare_short(1070), 0);
    }
}
