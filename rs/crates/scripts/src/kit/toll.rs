//! Travel coins (user rule 2026-11-17): a bot takes money for its trip
//! ONLY when the planned route really costs money, and only that amount.
//! Every fare is paid from the INVENTORY (`border_gate.rs2`: the Al
//! Kharid gate wants 10 coins in the pack; `sailors.rs2`,
//! `customs_officer.rs2`: the Karamja boat 30 each way), so a paid run
//! carries the fares its routes cross, decided by the nav plan
//! (`Bot::route_fare`: toll doors and fare edges, 0 for a free route):
//! - `arm_fares`: out (here → site) plus the way back (site → the site's
//!   bank), fetched before the first crossing;
//! - `bank_trip` / the combat restock: bank → site → bank, priced again
//!   for each new bank (`refresh_reserve`).
//! A run with neither `toll` nor `boat` keeps no travel coins: its bank
//! trips treat coins like any other item, and a route that asks for a
//! fare (the boat's `coins:60`) fetches exactly that from the nearest
//! bank (`fetch_fare_coins`, `skill_walk`).

#![forbid(unsafe_code)]

use lifescape_sdk::plan::Tile;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_slots, pack_total};
use crate::ScriptError;

/// Coins one trip takes, priced on its planned routes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TripFare {
    /// Fares of the way to the site.
    pub out: u32,
    /// Fares of the way back to the bank the site banks at.
    pub back: u32,
}

impl TripFare {
    /// Coins the pack carries when the trip starts: out and back, never
    /// a flat margin.
    pub fn carried(&self) -> u32 {
        self.out.saturating_add(self.back)
    }
}

fn tile((x, z, level): (i32, i32, i32)) -> Tile {
    Tile { x, z, level }
}

/// Fares of `from` → `site` → `bank` on the planned routes (fares
/// payable, `Bot::route_fare`).
pub fn trip_fare(bot: &mut Bot, from: (i32, i32, i32), site: (i32, i32, i32), bank: Option<(i32, i32, i32)>) -> Result<TripFare, ScriptError> {
    let out = bot.route_fare(tile(from), tile(site))?;
    let back = match bank {
        Some(bank) => bot.route_fare(tile(site), tile(bank))?,
        None => 0,
    };
    Ok(TripFare { out, back })
}

/// Coins the pack keeps for travel: the armed run's route-priced
/// reserve, none for any other run.
pub fn pack_reserve(bot: &Bot) -> u32 {
    if bot.allow_paid { bot.fare_reserve } else { 0 }
}

/// Coins to withdraw so the pack carries its reserve (`pack_reserve`).
pub fn reserve_short(bot: &Bot, carried: u32) -> u32 {
    pack_reserve(bot).saturating_sub(carried)
}

/// Pure: coins a bank trip withdraws AT the booth: the trip's own
/// request, raised to what the pack lacks of its `reserve` now (tour 3
/// point 8, felix_rook 14:10: the reserve was priced on Karamja before the
/// boat back paid 30, the bank topped 30 up to 30, and every walk out
/// planned NeedsRequirement coins:60 then fetched again).
pub fn top_up(requested: u32, reserve: u32, carried_now: u32) -> u32 {
    requested.max(reserve.saturating_sub(carried_now))
}

/// Close the run's paid paths and drop its reserve: a session's Bot
/// outlives one run, and the next run decides its own fares.
pub fn disarm(bot: &mut Bot) {
    bot.allow_paid = false;
    bot.fare_reserve = 0;
    bot.fare_bank = None;
}

/// Price the reserve of a paid run banking at `bank`: the fares of
/// bank → site → bank on the planned routes (the cycle every bank trip
/// starts). Once per bank: the same bank keeps its price. A run without
/// paid paths or without a site keeps what `arm_fares` set.
pub fn refresh_reserve(bot: &mut Bot, bank: (i32, i32, i32)) -> Result<(), ScriptError> {
    let Some(site) = bot.assigned_site.filter(|_| bot.allow_paid) else { return Ok(()) };
    if bot.fare_bank == Some(bank) {
        return Ok(());
    }
    let cycle = trip_fare(bot, bank, site, Some(bank))?;
    bot.fare_reserve = cycle.carried();
    bot.fare_bank = Some(bank);
    bot.log(&format!(
        "fares: bank ({},{}) -> site ({},{}) -> bank: {} + {} coins kept in the pack",
        bank.0, bank.1, site.0, site.1, cycle.out, cycle.back
    ));
    Ok(())
}

/// Coins a bank trip withdraws for the pack reserve, `keep` extended with
/// the coins of a paid run (they ride every trip, refilled to the
/// route-priced reserve). Any other run keeps no travel coins: the
/// caller's own request stands, coins bank like any item.
pub fn reserve_at_bank(bot: &mut Bot, bank: (i32, i32, i32), keep: &mut Vec<u16>, withdraw_coins: u32) -> Result<u32, ScriptError> {
    if !bot.allow_paid {
        return Ok(withdraw_coins);
    }
    let Ok(coins) = obj_id(bot, "coins") else { return Ok(withdraw_coins) };
    refresh_reserve(bot, bank)?;
    if !keep.contains(&coins) {
        keep.push(coins);
    }
    Ok(withdraw_coins.max(reserve_short(bot, pack_total(bot, coins))))
}

/// Coins a deposit-everything restock at `bank` takes back out so a paid
/// run still carries its route fares (frank_lane 2026-11-13: the combat
/// restock banked the fare, then "Yes, ok." at the gate with an empty
/// pack). Unpaid runs never withdraw.
pub fn restock_reserve(bot: &mut Bot, bank: (i32, i32, i32), carried_after_deposit: u32) -> Result<u32, ScriptError> {
    refresh_reserve(bot, bank)?;
    Ok(reserve_short(bot, carried_after_deposit))
}

/// `arm_fares` for a run that may only pay the toll.
pub fn arm(bot: &mut Bot, allow_paid: bool, site: Option<(i32, i32, i32)>) -> Result<(), ScriptError> {
    arm_fares(bot, allow_paid, false, site)
}

/// Opt the run into the crossings the brain budgeted (`toll`, `boat`)
/// and carry what its routes cost: here → `site` and site → the site's
/// bank, priced on the plan (0 when neither crosses a paid transition).
/// Without a site nothing is priced, so nothing is carried: a route that
/// asks for a fare fetches it on the way (`fetch_fare_coins`). Runs
/// with neither flag are untouched.
pub fn arm_fares(bot: &mut Bot, toll: bool, boat: bool, site: Option<(i32, i32, i32)>) -> Result<(), ScriptError> {
    if !toll && !boat {
        return Ok(());
    }
    crate::kit::survival::wait_pack_streamed(bot)?;
    let fare = match site {
        Some(site) => {
            let here = (bot.world.x, bot.world.z, bot.world.level);
            let bank = crate::kit::spots::nearest_bank_at(site.0, site.1, site.2).map(|bank| (bank.x as i32, bank.z as i32, bank.level as i32));
            trip_fare(bot, here, site, bank)?
        }
        None => TripFare::default(),
    };
    // The fares first, from a bank on this side (the pick is fenced while
    // paid paths are still off), then the crossings open to the run.
    bot.fare_reserve = fare.carried();
    bot.fare_bank = None;
    carry_fare(bot)?;
    bot.allow_paid = true;
    let what = match (toll, boat) {
        (true, true) => "toll gates and the Karamja boat",
        (false, true) => "the Karamja boat",
        _ => "toll gates",
    };
    bot.log(&format!(
        "{what} allowed for this run ({} coins carried; route fares: out {}, back {})",
        carried_coins(bot),
        fare.out,
        fare.back
    ));
    Ok(())
}

fn carried_coins(bot: &Bot) -> u32 {
    obj_id(bot, "coins").map(|coins| pack_total(bot, coins)).unwrap_or(0)
}

/// Coins to fetch so the pack carries `reserve` (0 when it does).
pub fn fare_shortfall(reserve: u32, carried: u32) -> u32 {
    reserve.saturating_sub(carried)
}

/// The arming reserve in the pack (`fare_reserve`, paid paths still
/// off): a bank trip withdraws the shortfall, the whole pack kept. Still
/// short after it (bank empty): typed `Blocked` on the coins the brain
/// files as a `have_coins` dependency.
fn carry_fare(bot: &mut Bot) -> Result<(), ScriptError> {
    let reserve = bot.fare_reserve;
    let short = fare_shortfall(reserve, carried_coins(bot));
    if short == 0 {
        return Ok(());
    }
    let keep: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    let bank = crate::kit::bank::nearest_bank(bot)?;
    bot.log(&format!("fares: fetching {short} coins from the bank at ({},{})", bank.x, bank.z));
    crate::kit::bank::bank_trip(bot, &bank, &keep, short)?;
    if fare_shortfall(reserve, carried_coins(bot)) > 0 {
        return Err(ScriptError::Blocked { missing: vec![format!("coins:{reserve}")] });
    }
    Ok(())
}

/// Fetch the coins a route's fare lacks (`Error::missing_coins`: the
/// Karamja boat wants `coins:60` in the pack) from the nearest bank, the
/// whole pack kept. `Ok(true)` once the pack carries `need`, so the walk
/// may plan again; `Ok(false)` when it already did (another cause) or the
/// bank had too few (the walk then fails with its own diagnosis).
pub fn fetch_fare_coins(bot: &mut Bot, need: u32) -> Result<bool, ScriptError> {
    let carried = carried_coins(bot);
    if carried >= need {
        return Ok(false);
    }
    let keep: Vec<u16> = pack_slots(bot).iter().map(|(_, (held, _))| *held).collect();
    let bank = crate::kit::bank::nearest_bank(bot)?;
    bot.log(&format!("fare: the route needs {need} coins in the pack, fetching {} at ({},{})", need - carried, bank.x, bank.z));
    crate::kit::bank::bank_trip(bot, &bank, &keep, need - carried)?;
    Ok(carried_coins(bot) >= need)
}

#[cfg(test)]
mod tests {
    use super::{disarm, fare_shortfall, pack_reserve, reserve_at_bank, reserve_short, restock_reserve, TripFare};

    fn live_bot() -> lifescape_sdk::Bot {
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        crate::tables_live::attach_live_tables(&mut bot);
        bot
    }

    fn carry(bot: &mut lifescape_sdk::Bot, coins: u32) -> u16 {
        let coins_id = crate::kit::pack::obj_id(bot, "coins").expect("coins");
        let pack_com = (bot.tables.com_id)("inventory:inv").expect("pack com");
        bot.world.inv.insert(pack_com, [(0u8, (coins_id, coins))].into_iter().collect());
        coins_id
    }

    /// A trip carries its route fares, nothing more: a free route 0, the
    /// Al Kharid gate out and back 2 x 10, the Karamja boat 30 + 30, the
    /// gate and the boat together 40 + 40.
    #[test]
    fn a_trip_carries_its_route_fares() {
        assert_eq!(TripFare { out: 0, back: 0 }.carried(), 0, "no toll, no boat: no coin");
        assert_eq!(TripFare { out: 10, back: 10 }.carried(), 20, "Al Kharid out and back");
        assert_eq!(TripFare { out: 10, back: 0 }.carried(), 10, "Al Kharid, banking on the far side");
        assert_eq!(TripFare { out: 30, back: 30 }.carried(), 60, "the Karamja boat");
        assert_eq!(TripFare { out: 40, back: 40 }.carried(), 80, "gate and boat both ways");
        assert_eq!(fare_shortfall(60, 25), 35);
        assert_eq!(fare_shortfall(0, 25), 0);
    }

    /// No paid run, no travel coins (user 2026-11-17): the 60-coin
    /// floor every bank trip withdrew is gone, coins bank like any item.
    #[test]
    fn an_unpaid_run_keeps_no_travel_coins() {
        let mut bot = live_bot();
        assert_eq!(pack_reserve(&bot), 0);
        assert_eq!(reserve_short(&bot, 0), 0);
        bot.fare_reserve = 60;
        assert_eq!(pack_reserve(&bot), 0, "a reserve without paid paths is never kept");
        let coins = carry(&mut bot, 500);
        let mut keep = Vec::new();
        assert_eq!(reserve_at_bank(&mut bot, (3092, 3245, 0), &mut keep, 0).expect("no plan"), 0, "a full wallet: nothing withdrawn");
        assert!(!keep.contains(&coins), "coins bank like any item");
        carry(&mut bot, 5);
        let mut keep = Vec::new();
        assert_eq!(reserve_at_bank(&mut bot, (3092, 3245, 0), &mut keep, 0).expect("no plan"), 0, "a few coins: nothing withdrawn");
        assert_eq!(reserve_at_bank(&mut bot, (3092, 3245, 0), &mut keep, 7).expect("no plan"), 7, "the caller's own request stands");
        assert_eq!(restock_reserve(&mut bot, (3092, 3245, 0), 0).expect("no plan"), 0, "unpaid restock withdraws nothing");
    }

    /// A paid run keeps its route-priced reserve: refilled, never more.
    /// Without a site nothing is priced (no plan asked).
    #[test]
    fn a_paid_run_keeps_its_priced_reserve() {
        let mut bot = live_bot();
        bot.allow_paid = true;
        bot.fare_reserve = 60;
        let coins = carry(&mut bot, 30);
        let mut keep = Vec::new();
        assert_eq!(reserve_at_bank(&mut bot, (3092, 3245, 0), &mut keep, 0).expect("no site, no plan"), 30, "back from Karamja: refill the way out");
        assert_eq!(keep, vec![coins], "the fares ride every trip");
        bot.fare_reserve = 0;
        assert_eq!(restock_reserve(&mut bot, (3092, 3245, 0), 0).expect("no plan"), 0, "a free cycle carries nothing");
        bot.fare_reserve = 10;
        assert_eq!(restock_reserve(&mut bot, (3092, 3245, 0), 4).expect("no plan"), 6);
        // Tour 3 point 8 (felix_rook): 30 asked on Karamja, the boat back
        // paid 30, the booth tops the pack up to the whole 60.
        assert_eq!(super::top_up(30, 60, 0), 60);
        assert_eq!(super::top_up(0, 60, 60), 0, "a full reserve withdraws nothing");
        assert_eq!(super::top_up(7, 60, 60), 7, "the caller's own request stands");
        assert_eq!(super::top_up(0, 0, 0), 0, "an unpaid run carries nothing");
        disarm(&mut bot);
        assert!(!bot.allow_paid && bot.fare_reserve == 0 && bot.fare_bank.is_none(), "the next run decides its own fares");
    }

    /// Same bank, same price: the reserve is priced once per bank (no
    /// plan asked again), a site-less run never plans.
    #[test]
    fn a_priced_bank_is_not_priced_again() {
        let mut bot = live_bot();
        bot.allow_paid = true;
        bot.assigned_site = Some((3300, 3310, 0));
        bot.fare_bank = Some((3269, 3167, 0));
        bot.fare_reserve = 0;
        super::refresh_reserve(&mut bot, (3269, 3167, 0)).expect("cached: no service needed");
        assert_eq!(bot.fare_reserve, 0);
        let error = super::refresh_reserve(&mut bot, (3092, 3245, 0)).expect_err("a new bank plans: no service here");
        assert!(error.to_string().contains("nav-down"), "{error}");
    }
}
