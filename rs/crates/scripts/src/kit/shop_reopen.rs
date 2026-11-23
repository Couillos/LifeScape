//! Shop sale that survives hits: the server closes every interface on each
//! hit the bot takes (`damage_self` runs `if_close`), so a keeper next to
//! aggressive NPCs (Bandit Camp) loses the shop mid-sale. The sale of a lot
//! reopens the shop and goes on, bounded; a failed sale still banks the
//! pack (what it paid and the lots it carries).

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_total};
use crate::kit::shop::{batch_sold, sell_batch_for, trade_to_keeper, units_paying, SELL_UNIT_TICKS};
use crate::ScriptError;

/// Why a run of sell clicks stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaleStop {
    /// Every asked unit sold.
    Done,
    /// The next unit pays under the minimum (the market): by design.
    Price,
    /// The shop interface is gone: the server closes every interface on
    /// each hit the bot takes (`damage_self` runs `if_close`), so a keeper
    /// next to aggressive NPCs (Bandit Camp) loses the shop mid-sale.
    ShopClosed,
    /// Any other stop (pause, nothing taken with the shop open).
    Other,
}

/// What a sale does once a lot's clicks stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterStop {
    NextLot,
    Reopen,
    /// Attackers around: kill them (`kit::shop_defense`), then reopen.
    FightThenReopen,
    GiveUp,
}

/// Reopenings of the shop one trip may spend on interruptions.
pub const SHOP_REOPEN_LIMIT: u32 = 30;

/// Pure: a closed shop with units still to sell is reopened (`reopens`
/// done so far, at most `SHOP_REOPEN_LIMIT`: then typed failure); every
/// other stop moves on to the next lot.
pub fn after_stop(stop: SaleStop, units_left: u32, reopens: u32, attackers: usize) -> AfterStop {
    match (stop, units_left) {
        (SaleStop::ShopClosed, left) if left > 0 => if reopens >= SHOP_REOPEN_LIMIT { AfterStop::GiveUp } else if attackers > 0 { AfterStop::FightThenReopen } else { AfterStop::Reopen },
        _ => AfterStop::NextLot,
    }
}

/// True while both halves of the shop view (its stock and the pack side)
/// are transmitted: a hit or any `if_close` stops both.
fn shop_view_open(bot: &Bot, main_com: u16, side_com: u16) -> bool {
    bot.world.inv.contains_key(&main_com) && bot.world.inv.contains_key(&side_com)
}

/// `sell_units` with the reason it stopped (`SaleStop`).
pub fn sell_units_stop(bot: &mut Bot, form: u16, obj_name: &str, shop: &str, count: u32, min_unit: i64) -> Result<(u32, i64, SaleStop), ScriptError> {
    let side_com = (bot.tables.com_id)("shop_template_side:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template_side:inv missing from component table".to_string()))?;
    let main_com = (bot.tables.com_id)("shop_template:inv")
        .ok_or_else(|| ScriptError::Usage("shop_template:inv missing from component table".to_string()))?;
    let coins = obj_id(bot, "coins")?;
    let base = obj_id(bot, obj_name)?;
    let (mut sold, mut earned) = (0u32, 0i64);
    let mut stop = SaleStop::Done;
    while sold < count {
        if !shop_view_open(bot, main_com, side_com) {
            bot.log(&format!("shopsell {obj_name}: the shop closed after {sold} (a hit closes every interface) at {:?}", bot.position()));
            stop = SaleStop::ShopClosed;
            break;
        }
        let shop_holds: i64 = bot.world.inv.get(&main_com).map(|slots| slots.values().filter(|(held, _)| *held == base).map(|(_, qty)| i64::from(*qty)).sum()).unwrap_or(0);
        let Some(paying) = units_paying(shop, obj_name, shop_holds, count - sold, min_unit) else {
            return Err(ScriptError::Usage(format!("shop {shop} buys no {obj_name}")));
        };
        if paying == 0 {
            bot.log(&format!("shopsell {obj_name}: the next unit pays under {min_unit} (the market): stop after {sold}"));
            stop = SaleStop::Price;
            break;
        }
        let units_before = pack_total(bot, form);
        // Both stops are logged: 3 294 lots of the day ended "sold 0"
        // with no line saying why (lot 8, cause still open).
        let Some((button, size)) = sell_batch_for(paying.min(units_before)) else {
            bot.log(&format!("shopsell {obj_name}: no unit of form {form} in the pack view after {sold} (paying {paying}) at {:?}", bot.position()));
            stop = SaleStop::Other;
            break;
        };
        let Some(slot) = bot.world.inv.get(&side_com).and_then(|slots| slots.iter().find(|(_, (held, _))| *held == form).map(|(slot, _)| *slot)) else {
            let side = bot.world.inv.get(&side_com).map(|slots| slots.len());
            bot.log(&format!("shopsell {obj_name}: form {form} not in the shop's side inventory ({side:?} slots) after {sold} at {:?}", bot.position()));
            stop = SaleStop::Other;
            break;
        };
        let coins_before = pack_total(bot, coins);
        let pauses_before = bot.pauses_done;
        bot.inv_button(button, form, slot, side_com)?;
        let deadline = bot.world.ticks + SELL_UNIT_TICKS;
        while bot.world.ticks < deadline && pack_total(bot, form) >= units_before && bot.pauses_done == pauses_before {
            bot.pump(1)?;
        }
        let Some(gone) = batch_sold(units_before, pack_total(bot, form), pauses_before, bot.pauses_done) else {
            bot.log(&format!("shopsell {obj_name}: a market pause cut the sale after {sold} at {:?}", bot.position()));
            stop = SaleStop::Other;
            break;
        };
        if gone == 0 {
            bot.log(&format!("shopsell {obj_name}: the shop took nothing of a {size} batch at {:?}", bot.position()));
            stop = if shop_view_open(bot, main_com, side_com) { SaleStop::Other } else { SaleStop::ShopClosed };
            break;
        }
        bot.pump(1)?;
        sold += gone;
        if bot.pauses_done != pauses_before {
            // Sold before the pause, its coins banked by it: no coin read.
            bot.log(&format!("shopsell {obj_name}: a market pause followed the batch after {sold} at {:?}", bot.position()));
            stop = SaleStop::Other;
            break;
        }
        earned += i64::from(pack_total(bot, coins)) - i64::from(coins_before);
    }
    Ok((sold, earned, stop))
}

/// Sells one lot (`qty` units carried as `form`), reopening the shop each
/// time a hit closes it. `pause_seen` tells a market pause happened (the
/// pack was banked: the lot stops there). Returns (units sold, coins).
pub fn sell_lot_reopening(bot: &mut Bot, keeper: &str, spawn: (i32, i32, i32), form: u16, lot: &crate::activities::market_shop_sell::SaleLot, shop: &str, reopens: &mut u32, pause_seen: &dyn Fn(&Bot) -> bool) -> Result<(u32, i64), ScriptError> {
    let (mut sold, mut coins) = (0u32, 0i64);
    loop {
        let (batch, earned, stop) = sell_units_stop(bot, form, &lot.item, shop, lot.qty - sold, lot.min_unit)?;
        sold += batch;
        coins += earned;
        if pause_seen(bot) {
            return Ok((sold, coins));
        }
        let after = after_stop(stop, lot.qty - sold, *reopens, crate::kit::shop_defense::attackers(bot).len());
        match after {
            AfterStop::NextLot => return Ok((sold, coins)),
            AfterStop::GiveUp => return Err(ScriptError::Failed(format!("shopsell: the shop of {keeper} closed {SHOP_REOPEN_LIMIT} times (attacks) at {:?}, {sold} of {} {} sold", bot.position(), lot.qty, lot.item))),
            AfterStop::Reopen | AfterStop::FightThenReopen => {
                if after == AfterStop::FightThenReopen {
                    crate::kit::shop_defense::defend(bot, &["brawling_bandit", "guard_bandit"])?;
                }
                *reopens += 1;
                bot.log(&format!("shopsell {}: the shop closed after {sold}/{}, reopening ({}/{SHOP_REOPEN_LIMIT})", lot.item, lot.qty, *reopens));
                if !trade_to_keeper(bot, keeper, spawn)? {
                    return Err(ScriptError::Failed(format!("shopsell: trade with {keeper} failed again at {:?}", bot.position())));
                }
            }
        }
    }
}

/// After a failed sale: banks the pack (`bank_leftovers`) unless the
/// session is dead (socket lost, bot dead); a banking failure is logged,
/// the sale's own error stays the one reported.
pub fn bank_after_failed_sale(bot: &mut Bot, error: &ScriptError) {
    if crate::kit::survival::is_conn_abort(error) || crate::kit::survival::is_death_abort(error) {
        return;
    }
    let outcome = (|| -> Result<(), ScriptError> {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
            return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
        }
        crate::market_lots::bank_leftovers(bot)?;
        let _ = bot.close_modal();
        Ok(())
    })();
    if let Err(banking) = outcome {
        bot.log(&format!("shopsell: banking after the failed sale did not work ({banking})"));
    }
}

#[cfg(test)]
mod tests {
    use crate::tables_live::attach_live_tables;
    use lifescape_sdk::Bot;

    /// Live 2026-11-22/23 (Bandit Camp, 24 h: 336 of 734 sale trips to
    /// `taxfree` cut by "form not in the shop's side inventory"): every
    /// hit runs `if_close` (`damage_self`), the server stops the shop's
    /// inventories, and the sale used to end there. A closed view stops
    /// with `ShopClosed`, never a price stop; the sale then reopens.
    #[test]
    fn a_shop_closed_by_a_hit_is_told_apart_from_a_price_stop() {
        use super::{after_stop, sell_units_stop, AfterStop, SaleStop, SHOP_REOPEN_LIMIT};
        let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
        attach_live_tables(&mut bot);
        let main_com = (bot.tables.com_id)("shop_template:inv").expect("main com");
        bot.world.inv.insert(main_com, Default::default());
        let (sold, earned, stop) = sell_units_stop(&mut bot, 1516, "yew_logs", "taxfree", 22, 9).expect("no network needed");
        assert_eq!((sold, earned, stop), (0, 0, SaleStop::ShopClosed), "the side inventory is gone");
        assert_eq!(after_stop(SaleStop::ShopClosed, 12, 0, 0), AfterStop::Reopen, "units left: the shop reopens");
        assert_eq!(after_stop(SaleStop::ShopClosed, 12, 3, 2), AfterStop::FightThenReopen, "attacked: kill, then reopen");
        assert_eq!(after_stop(SaleStop::ShopClosed, 0, 0, 2), AfterStop::NextLot, "lot done");
        assert_eq!(after_stop(SaleStop::Price, 12, 0, 2), AfterStop::NextLot, "the market pays more: by design");
        assert_eq!(after_stop(SaleStop::Other, 12, 0, 0), AfterStop::NextLot);
        assert_eq!(after_stop(SaleStop::ShopClosed, 12, SHOP_REOPEN_LIMIT, 1), AfterStop::GiveUp, "bounded");
    }
}
