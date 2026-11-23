//! Lot withdrawal and boat fare of the NPC shop sale (`market cleanup`
//! shop trip): split out of the thin activity.

#![forbid(unsafe_code)]

use lifescape_sdk::Bot;

use crate::kit::pack::obj_id;
use crate::ScriptError;

/// Pure: coins the trip to a keeper at `site` pays in fares: the Karamja
/// boat out and back (the nav's `coins:60` on `boat_port_sarim_to_karamja`,
/// the brain's `payable_sale_sites` check), nothing on the mainland (the
/// cleanup run never opens the toll gates).
pub fn sale_fare_coins(site: (i32, i32, i32)) -> u32 {
    let (x, z, level) = site;
    if lifescape_gamedata::access::behind_boat(i64::from(x), i64::from(z), i64::from(level)) {
        (2 * lifescape_gamedata::services::BOAT_FARE_GP) as u32
    } else {
        0
    }
}

/// Pure: coins to withdraw with the lots so the pack (`carried` coins)
/// pays the trip's fares (`sale_fare_coins`).
pub fn fare_to_withdraw(site: (i32, i32, i32), carried: u32) -> u32 {
    sale_fare_coins(site).saturating_sub(carried)
}

/// Takes every lot into the pack (banknotes for non-stackables: one slot
/// each), only the units not carried yet, plus the coins the trip to
/// `site` pays in fares (`fare_to_withdraw`: before the first walk, never
/// after a `NeedsRequirement coins:60`); returns the lots carried with
/// their form (empty: nothing left to sell).
pub fn withdraw_lots(bot: &mut Bot, lots: &[crate::activities::market_shop_sell::SaleLot], site: (i32, i32, i32)) -> Result<Vec<(crate::activities::market_shop_sell::SaleLot, u16)>, ScriptError> {
    let mut wanted: Vec<(&crate::activities::market_shop_sell::SaleLot, u16, u16, u32)> = Vec::new();
    for lot in lots {
        let base = obj_id(bot, &lot.item)?;
        let note = crate::market_client::trade_id(base);
        let carried = crate::kit::pack::pack_total(bot, base) + if note != base { crate::kit::pack::pack_total(bot, note) } else { 0 };
        wanted.push((lot, base, note, lot.qty.saturating_sub(carried)));
    }
    let coins = obj_id(bot, "coins")?;
    let fare = fare_to_withdraw(site, crate::kit::pack::pack_total(bot, coins));
    if fare > 0 || wanted.iter().any(|(_, _, _, missing)| *missing > 0) {
        let bank = crate::kit::bank::nearest_bank(bot)?;
        if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300)? {
            return Err(ScriptError::Blocked { missing: vec!["bank".to_string()] });
        }
        bot.pump(2)?;
        if fare > 0 {
            bot.log(&format!("shopsell: the trip to ({},{}) pays the boat, taking {fare} coins with the lots", site.0, site.1));
            if let Err(error) = bot.bank_withdraw(coins, fare, 300) {
                bot.log(&format!("shopsell: fare withdraw failed ({error})"));
            }
        }
        for (lot, base, note, missing) in &wanted {
            let banked = bot.bank_items().get(base).copied().unwrap_or(0);
            let take = (*missing).min(banked);
            if take > 0 {
                let outcome = if note != base { bot.bank_withdraw_noted(*base, *note, take, 300) } else { bot.bank_withdraw(*base, take, 300) };
                if let Err(error) = outcome {
                    bot.log(&format!("shopsell {}: withdraw failed ({error})", lot.item));
                }
            }
        }
        let _ = bot.close_modal();
        bot.pump(3)?;
    }
    let mut forms: Vec<(crate::activities::market_shop_sell::SaleLot, u16)> = Vec::new();
    for (lot, base, note, _) in wanted {
        let form = if crate::kit::pack::pack_total(bot, note) > 0 { note } else { base };
        let carried = crate::kit::pack::pack_total(bot, form).min(lot.qty);
        if carried > 0 {
            forms.push((crate::activities::market_shop_sell::SaleLot { qty: carried, ..lot.clone() }, form));
        }
    }
    Ok(forms)
}
