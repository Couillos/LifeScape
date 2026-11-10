//! Market deliver: physical handovers on the Varrock square.
//! Split from `market.rs` so every activity stays under the plafond.

#![forbid(unsafe_code)]

use lifescape_json::{str_value, Value};
use lifescape_sdk::Bot;

use crate::market_client::{buyer_want, deal_report, ensure_goods, market_call, my_deals, seller_form, trade_id};
use crate::kit::bank::nearest_bank;
use crate::kit::pack::obj_id;
use crate::{ScriptError, ScriptResult};

/// `market deliver <depart> <deadline> <deal>=<x>,<y> ...`: bank when
/// short, wait for the staggered departure, walk to the tile, trade with
/// a double-screen check, redeposit, walk back to the start tile.
pub fn run_deliver(bot: &mut Bot, args: &[String]) -> ScriptResult {
    if args.len() < 4 {
        return Err(ScriptError::Usage("market deliver <depart> <deadline> <deal>=<x>,<y> ...".to_string()));
    }
    let depart: u64 = args[1].parse().map_err(|_| ScriptError::Usage("depart must be a tick".to_string()))?;
    let deadline: u64 = args[2].parse().map_err(|_| ScriptError::Usage("deadline must be a tick".to_string()))?;
    let mut jobs: Vec<(i64, i32, i32)> = Vec::new();
    for token in &args[3..] {
        let (id, tile) = token.split_once('=').ok_or_else(|| ScriptError::Usage(format!("bad deal token {token:?}")))?;
        let (x, z) = tile.split_once(',').ok_or_else(|| ScriptError::Usage(format!("bad tile {tile:?}")))?;
        jobs.push((id.parse().map_err(|_| ScriptError::Usage(format!("bad deal {id:?}")))?, x.parse().map_err(|_| ScriptError::Usage(format!("bad x {x:?}")))?, z.parse()
            .map_err(|_| ScriptError::Usage(format!("bad z {z:?}")))?));
    }
    let start = bot.position();
    let rows = my_deals(&bot.username)?;
    let coins = obj_id(bot, "coins")?;
    let mut touched = vec![coins];
    let mut ok = true;
    for (id, x, z) in jobs {
        let Some(row) = rows.iter().find(|row| row.id == id) else {
            bot.log(&format!("deal {id} is not mine, skipping"));
            ok = false;
            continue;
        };
        let trade = trade_id(row.item);
        touched.push(trade);
        if !ensure_goods(bot, row, trade, coins)? {
            bot.log(&format!("deal {id}: lots short, reporting no_show"));
            deal_report(&bot.username, id, "failed", "no_show");
            ok = false;
            continue;
        }
        while bot.world.ticks < depart && bot.world.ticks < deadline {
            bot.pump(5)?;
        }
        if bot.world.ticks >= deadline {
            deal_report(&bot.username, id, "failed", "timeout");
            ok = false;
            continue;
        }
        if bot.travel_to(lifescape_sdk::plan::Tile { x, z, level: 0 }).is_err() {
            bot.log(&format!("deal {id}: square unreachable"));
            deal_report(&bot.username, id, "failed", "timeout");
            ok = false;
            continue;
        }
        let _ = market_call("deal_arrived", vec![("account", str_value(&bot.username)), ("deal", Value::Int(id))]);
        // Mutual knocks open the window (one-sided requests sit as
        // "Sending..." until the other side knocks back).
        let mut knocks = 0u32;
        while bot.world.ticks < deadline && bot.trade_modal().is_none() {
            let left = deadline.saturating_sub(bot.world.ticks).min(600).max(1);
            if bot.find_player(&row.partner).is_none() && knocks % 20 == 0 {
                bot.log(&format!(
                    "deal {id}: cannot see {} ({} players tracked)",
                    row.partner,
                    bot.world.players.len()
                ));
            }
            knocks += 1;
            if bot.trade_request(&row.partner, left)? {
                break;
            }
        }
        if bot.trade_modal().is_none() {
            bot.log(&format!("deal {id}: partner absent"));
            deal_report(&bot.username, id, "failed", "no_show");
            ok = false;
            continue;
        }
        deal_report(&bot.username, id, "trading", "");
        let coins_n = (i64::from(row.qty) * row.price) as u32;
        // Seller lots: the carried form. Buyer lots: whatever the seller
        // put up, validated as the deal item at the exact quantity.
        let (give, want) = if row.buying {
            while bot.world.ticks < deadline
                && bot.trade_modal() == Some("main")
                && bot.trade_partner_offer().is_empty()
            {
                bot.pump(5)?;
            }
            let observed = bot.trade_partner_offer();
            let Some(lots) = buyer_want(&observed, row.item, trade, row.qty) else {
                bot.log(&format!("deal {id}: offer mismatch, refused"));
                deal_report(&bot.username, id, "failed", "mismatch");
                ok = false;
                continue;
            };
            (vec![(coins, coins_n)], lots)
        } else {
            let Some(form) = seller_form(bot, row, trade) else {
                bot.log(&format!("deal {id}: lots gone"));
                deal_report(&bot.username, id, "failed", "timeout");
                ok = false;
                continue;
            };
            (vec![form], vec![(coins, coins_n)])
        };
        let left = deadline.saturating_sub(bot.world.ticks).max(1);
        match bot.trade_exchange(&row.partner, &give, 0, &want, 0, left.min(3000))? {
            lifescape_sdk::client::actions::ExchangeOutcome::Settled => {
                bot.log(&format!("deal {id} settled"));
                deal_report(&bot.username, id, "settled", "");
            }
            lifescape_sdk::client::actions::ExchangeOutcome::PartnerMismatch
            | lifescape_sdk::client::actions::ExchangeOutcome::ConfirmMismatch => {
                bot.log(&format!("deal {id}: offer mismatch, refused"));
                deal_report(&bot.username, id, "failed", "mismatch");
                ok = false;
            }
            other => {
                bot.log(&format!("deal {id} ended: {other:?}"));
                deal_report(&bot.username, id, "failed", "timeout");
                ok = false;
            }
        }
    }
    // Best-effort redeposit of the traded stacks (notes land back as
    // base stock), then back to the start tile. Loud when it fails: a
    // silent drop here loses the lots with no trace.
    if let Ok(bank) = nearest_bank(bot) {
        if bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300).unwrap_or(false) {
            if let Some(side_com) = (bot.tables.com_id)("bank_side:inv") {
                let carried: Vec<u16> = bot.backpack().1.iter().map(|(_, (held, _))| *held).collect();
                let keep: Vec<u16> = carried.into_iter().filter(|held| !touched.contains(held)).collect();
                match bot.bank_deposit_all(side_com, &keep, 300) {
                    Ok(true) => {}
                    other => bot.log(&format!("redeposit outcome: {other:?}")),
                }
            }
            let _ = bot.close_modal();
        } else {
            bot.log("redeposit skipped: bank did not open");
        }
    }
    let (level, x, z) = start;
    let _ = bot.travel_to(lifescape_sdk::plan::Tile { x, z, level });
    Ok(if ok { 0 } else { 1 })
}
