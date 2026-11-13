//! Fares the brain budgeted (user decision 2026-11-13): the run may pay a
//! crossing, and every fare is taken from the INVENTORY, never the bank.
//! - `toll`: the Al Kharid border gate, 10 coins (`border_gate.rs2`:
//!   `inv_total(inv, coins) < 10` ⇒ "Oh dear, I don't actually seem to
//!   have enough money");
//! - `boat`: the Port Sarim sailors and the Karamja customs officer, 30
//!   coins each way (`sailors.rs2`, `customs_officer.rs2`): the run keeps
//!   a round trip (60) so a bot on Karamja always sails back.
//! A bot whose coins sit in the bank fetches the reserve before the first
//! crossing; bank trips of a paid run keep it and refill to it
//! (`kit::bank::bank_trip`).

#![forbid(unsafe_code)]

use lifescape_gamedata::services::BOAT_FARE_GP;
use lifescape_gamedata::toll::FARE;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_slots, pack_total};
use crate::ScriptError;

/// Coins a toll run keeps in the pack.
pub fn fare() -> u32 {
    FARE as u32
}

/// Coins a boat run keeps in the pack: out and back.
pub fn boat_round_trip() -> u32 {
    2 * BOAT_FARE_GP as u32
}

/// Pack reserve of a run allowed these crossings.
pub fn reserve(toll: bool, boat: bool) -> u32 {
    (if toll { fare() } else { 0 }) + if boat { boat_round_trip() } else { 0 }
}

/// Coins to withdraw so the pack carries the toll fare (0 when it does).
pub fn fare_short(carried: u32) -> u32 {
    fare().saturating_sub(carried)
}

/// Coins to withdraw so the pack carries the run's reserve: the armed
/// reserve, else the toll fare for a run that only set `allow_paid`.
pub fn reserve_short(bot: &Bot, carried: u32) -> u32 {
    let reserve = if bot.fare_reserve > 0 { bot.fare_reserve } else { fare() };
    reserve.saturating_sub(carried)
}

/// Coins a deposit-everything restock must take back out so a paid run
/// still carries the fare at the gate (frank_lane 2026-11-13: the combat
/// restock banked the fare `arm` had just fetched, then "Yes, ok." at the
/// gate with an empty pack, looped). Unpaid runs never withdraw.
pub fn restock_fare(allow_paid: bool, carried_after_deposit: u32) -> u32 {
    if allow_paid { fare_short(carried_after_deposit) } else { 0 }
}

/// `restock_fare` for a live run: the armed reserve (a boat run keeps
/// its round trip), the toll fare for a bare `allow_paid` run.
pub fn restock_reserve(bot: &Bot, carried_after_deposit: u32) -> u32 {
    if bot.allow_paid { reserve_short(bot, carried_after_deposit) } else { 0 }
}

/// Opt the run into paid crossings (`toll`) and carry the fare. Unpaid
/// runs are untouched.
pub fn arm(bot: &mut Bot, allow_paid: bool) -> Result<(), ScriptError> {
    arm_fares(bot, allow_paid, false)
}

/// Opt the run into the crossings the brain budgeted (`toll`, `boat`)
/// and carry their reserve in the pack. Runs with neither are untouched.
pub fn arm_fares(bot: &mut Bot, toll: bool, boat: bool) -> Result<(), ScriptError> {
    if !toll && !boat {
        return Ok(());
    }
    // The reserve first, from a bank on this side (the pick is fenced
    // while paid paths are still off), then the crossings open to the run.
    bot.fare_reserve = reserve(toll, boat);
    carry_fare(bot)?;
    bot.allow_paid = true;
    let what = match (toll, boat) {
        (true, true) => "toll gates and the Karamja boat",
        (false, true) => "the Karamja boat",
        _ => "toll gates",
    };
    bot.log(&format!("{what} allowed for this run ({} coins carried, reserve {})", carried_coins(bot), bot.fare_reserve));
    Ok(())
}

fn carried_coins(bot: &Bot) -> u32 {
    obj_id(bot, "coins").map(|coins| pack_total(bot, coins)).unwrap_or(0)
}

/// Reserve in the pack: a bank trip withdraws the shortfall, the whole
/// pack kept. Still short after it (bank empty): typed `Blocked` on the
/// coins the brain files as a `have_coins` dependency.
pub fn carry_fare(bot: &mut Bot) -> Result<(), ScriptError> {
    crate::kit::survival::wait_pack_streamed(bot)?;
    let short = reserve_short(bot, carried_coins(bot));
    if short == 0 {
        return Ok(());
    }
    let keep: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    let bank = crate::kit::bank::nearest_bank(bot)?;
    bot.log(&format!("fares: fetching {short} coins from the bank at ({},{})", bank.x, bank.z));
    crate::kit::bank::bank_trip(bot, &bank, &keep, short)?;
    if reserve_short(bot, carried_coins(bot)) > 0 {
        let reserve = if bot.fare_reserve > 0 { bot.fare_reserve } else { fare() };
        return Err(ScriptError::Blocked { missing: vec![format!("coins:{reserve}")] });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{boat_round_trip, fare, fare_short, reserve, reserve_short, restock_fare};

    #[test]
    fn a_paid_restock_takes_the_fare_back_out() {
        assert_eq!(restock_fare(true, 0), 10, "deposit-all banked the fare");
        assert_eq!(restock_fare(true, 25), 0);
        assert_eq!(restock_fare(false, 0), 0, "unpaid runs carry nothing");
    }

    #[test]
    fn the_fare_is_ten_coins_carried() {
        assert_eq!(fare(), 10);
        assert_eq!(fare_short(0), 10);
        assert_eq!(fare_short(4), 6);
        assert_eq!(fare_short(1070), 0);
    }

    /// The boat keeps a round trip (30 out, 30 back); a run allowed both
    /// crossings carries both; a bare `allow_paid` run keeps the toll.
    #[test]
    fn the_boat_keeps_a_round_trip() {
        assert_eq!(boat_round_trip(), 60);
        assert_eq!((reserve(true, false), reserve(false, true), reserve(true, true), reserve(false, false)), (10, 60, 70, 0));
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        assert_eq!(reserve_short(&bot, 3), 7, "toll-only runs keep the gate fare");
        bot.fare_reserve = reserve(false, true);
        assert_eq!(reserve_short(&bot, 30), 30, "back from Karamja: refill the way out");
        assert_eq!(reserve_short(&bot, 75), 0);
    }
}
