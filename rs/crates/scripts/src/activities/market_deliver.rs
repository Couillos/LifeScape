//! Market deliver: physical handovers on the Varrock square.
//! Split from `market.rs` so every activity stays under the plafond.

#![forbid(unsafe_code)]

use lifescape_json::{str_value, Value};
use lifescape_sdk::Bot;

use crate::market_client::{buyer_want, deal_report, market_call, my_deals, trade_id};
use crate::market_lots::{group_deals, secure_lots, DealGroup};
use crate::kit::pack::pack_total;
use crate::kit::bank::nearest_bank;
use crate::kit::pack::obj_id;
use crate::{ScriptError, ScriptResult};

/// Pack ids to redeposit: everything carried except the untouched.
/// The touched set must hold every dealt form (base AND note): the
/// seller's window form may differ from the trade id, and a form kept
/// by mistake never returns to the bank.
pub fn redeposit_keep(carried: &[u16], touched: &[u16]) -> Vec<u16> {
    carried.iter().copied().filter(|held| !touched.contains(held)).collect()
}
/// Past its tick deadline a party keeps waiting for a late partner while
/// the daemon keeps the deal open, at most this many ticks more.
pub const WAIT_PAST_DEADLINE_TICKS: u64 = 3000;

/// True while the daemon still lists one of `ids` open for this bot and
/// the hard cap past `deadline` is not reached.
fn deal_alive(bot: &Bot, ids: &[i64], deadline: u64) -> bool {
    if bot.world.ticks >= deadline + WAIT_PAST_DEADLINE_TICKS {
        return false;
    }
    match my_deals(&bot.username) {
        Ok(rows) => rows.iter().any(|row| ids.contains(&row.id)),
        Err(_) => false,
    }
}

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
    // One exchange per partner and object, in ascending deal order on both
    // sides (user protocol 2026-11-13, see `market_lots`).
    let (groups, unknown) = group_deals(&jobs, &rows);
    for id in unknown {
        bot.log(&format!("deal {id} is not mine, skipping"));
    }
    let mut touched = vec![coins];
    for group in &groups {
        let trade = trade_id(group.item);
        touched.push(trade);
        if group.item != trade {
            touched.push(group.item);
        }
    }
    let report_all = |bot: &Bot, group: &DealGroup, outcome: &str, reason: &str| {
        for id in &group.ids {
            deal_report(&bot.username, *id, outcome, reason);
        }
    };
    let mut ok = true;
    // Nearest bank in, Varrock west out: the lots of every group at once.
    let secured = secure_lots(bot, &groups, coins)?;
    let Some(flags) = secured else {
        for group in &groups {
            report_all(bot, group, "failed", "timeout");
        }
        return Ok(1);
    };
    for (group, covered) in groups.iter().zip(flags) {
        let ids = &group.ids;
        if !covered {
            // `short`, not `no_show`: the goods (or the coins) are not
            // there at all. The daemon cancels this side's orders instead
            // of relisting phantom stock that would fail forever.
            bot.log(&format!("deals {ids:?}: lots short, reporting short"));
            report_all(bot, group, "failed", "short");
            ok = false;
            continue;
        }
        while bot.world.ticks < depart && bot.world.ticks < deadline {
            bot.pump(5)?;
        }
        // A late pose (the gate waits for the safe point) arrives with its
        // tick deadline half burnt: past it, the deal still goes as long
        // as the daemon keeps it open (it is the arbiter: partner failure
        // or sweep close it), up to a hard cap.
        if bot.world.ticks >= deadline && !deal_alive(bot, &group.ids, deadline) {
            report_all(bot, group, "failed", "timeout");
            ok = false;
            continue;
        }
        let (x, z) = group.tile;
        if bot.travel_to(lifescape_sdk::plan::Tile { x, z, level: 0 }).is_err() {
            bot.log(&format!("deals {ids:?}: square unreachable"));
            report_all(bot, group, "failed", "timeout");
            ok = false;
            continue;
        }
        for id in ids {
            let _ = market_call("deal_arrived", vec![("account", str_value(&bot.username)), ("deal", Value::Int(*id))]);
        }
        // Mutual knocks open the window (one-sided requests sit as
        // "Sending..." until the other side knocks back).
        let mut knocks = 0u32;
        let mut last_check = bot.world.ticks;
        while bot.trade_modal().is_none() {
            // Wait for a late partner while the deal lives (checked every
            // ~100 ticks against the daemon), never past the hard cap.
            if bot.world.ticks >= deadline && bot.world.ticks.saturating_sub(last_check) >= 100 {
                last_check = bot.world.ticks;
                if !deal_alive(bot, &group.ids, deadline) {
                    break;
                }
            }
            if bot.world.ticks >= deadline + WAIT_PAST_DEADLINE_TICKS {
                break;
            }
            let left = (deadline + WAIT_PAST_DEADLINE_TICKS).saturating_sub(bot.world.ticks).min(600).max(1);
            if bot.find_player(&group.partner).is_none() && knocks % 20 == 0 {
                bot.log(&format!(
                    "deals {ids:?}: cannot see {} ({} players tracked)",
                    group.partner,
                    bot.world.players.len()
                ));
            }
            knocks += 1;
            if bot.trade_request(&group.partner, left)? {
                break;
            }
        }
        if bot.trade_modal().is_none() {
            bot.log(&format!("deals {ids:?}: partner absent"));
            report_all(bot, group, "failed", "no_show");
            ok = false;
            continue;
        }
        report_all(bot, group, "trading", "");
        let trade = trade_id(group.item);
        // Seller lots: the carried form. Buyer lots: whatever the seller
        // put up, validated as the deal item at the exact summed quantity.
        let (give, want) = if group.buying {
            while bot.world.ticks < deadline
                && bot.trade_modal() == Some("main")
                && bot.trade_partner_offer().is_empty()
            {
                bot.pump(5)?;
            }
            let observed = bot.trade_partner_offer();
            let Some(lots) = buyer_want(&observed, group.item, trade, group.qty) else {
                bot.log(&format!("deals {ids:?}: offer mismatch ({observed:?} for {} x{}), refused", group.item, group.qty));
                report_all(bot, group, "failed", "mismatch");
                ok = false;
                continue;
            };
            (vec![(coins, group.coins)], lots)
        } else {
            let form = if pack_total(bot, group.item) >= group.qty { group.item } else { trade };
            if pack_total(bot, form) < group.qty {
                bot.log(&format!("deals {ids:?}: lots gone"));
                report_all(bot, group, "failed", "short");
                ok = false;
                continue;
            }
            (vec![(form, group.qty)], vec![(coins, group.coins)])
        };
        let left = (deadline + WAIT_PAST_DEADLINE_TICKS).saturating_sub(bot.world.ticks).max(1);
        match bot.trade_exchange(&group.partner, &give, 0, &want, 0, left.max(600).min(3000))? {
            lifescape_sdk::client::actions::ExchangeOutcome::Settled => {
                bot.log(&format!("deals {ids:?} settled ({} x{} for {} coins)", group.item, group.qty, group.coins));
                report_all(bot, group, "settled", "");
            }
            lifescape_sdk::client::actions::ExchangeOutcome::PartnerMismatch
            | lifescape_sdk::client::actions::ExchangeOutcome::ConfirmMismatch => {
                bot.log(&format!("deals {ids:?}: offer mismatch, refused"));
                report_all(bot, group, "failed", "mismatch");
                ok = false;
            }
            other => {
                bot.log(&format!("deals {ids:?} ended: {other:?}"));
                report_all(bot, group, "failed", "timeout");
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
                let keep: Vec<u16> = redeposit_keep(&carried, &touched);
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

#[cfg(test)]
mod tests {
    use super::redeposit_keep;

    /// Deal 24 regression: the seller delivered base willow (1519) while
    /// the trade id is the note (1520) — both forms must redeposit.
    #[test]
    fn redeposit_covers_both_dealt_forms() {
        let carried = [995u16, 1519, 314];
        let touched = [995u16, 1520, 1519];
        assert_eq!(redeposit_keep(&carried, &touched), vec![314u16]);
    }
}
