//! Market deliver: physical handovers on the Varrock square.
//! Split from `market.rs` so every activity stays under the plafond;
//! the handover mechanics live in `market_handover`.

#![forbid(unsafe_code)]

use lifescape_json::{str_value, Value};
use lifescape_sdk::client::actions::ExchangeOutcome;
use lifescape_sdk::Bot;

use crate::kit::pack::{obj_id, pack_total};
use crate::market_client::{market_call, my_deals, trade_id};
use crate::market_handover::{
    meet_partner, parse_deliver_args, redeposit, seller_offer, wait_departure, DealWatch, DeliverReports, SellerOffer,
    MEET_RETRIES, WAIT_PAST_DEADLINE_TICKS,
};
use crate::market_lots::{group_deals, secure_lots, DealGroup};
use crate::{ScriptError, ScriptResult};

/// `market deliver <depart> <deadline> <deal>=<x>,<y> ...`: bank when
/// short, wait for the staggered departure, walk to the tile, trade with
/// a double-screen check, redeposit, walk back to the start tile. Every
/// posed deal gets a terminal report, error paths included: a run that
/// died after `trading` left the deal stuck there forever (finding 2).
pub fn run_deliver(bot: &mut Bot, args: &[String]) -> ScriptResult {
    let (depart, deadline, jobs) = parse_deliver_args(args)?;
    let posed: Vec<i64> = jobs.iter().map(|job| job.0).collect();
    let mut reports = DeliverReports::new(&bot.username);
    let outcome = deliver_groups(bot, depart, deadline, &jobs, &mut reports);
    if let Err(error) = &outcome {
        bot.log(&format!("deliver aborted: {error}"));
        reports.flush_unfinished(bot, &posed);
    }
    outcome
}

fn deliver_groups(
    bot: &mut Bot,
    depart: u64,
    deadline: u64,
    jobs: &[(i64, i32, i32)],
    reports: &mut DeliverReports,
) -> ScriptResult {
    let start = bot.position();
    let rows = my_deals(&bot.username)?;
    let coins = obj_id(bot, "coins")?;
    // One exchange per partner and object, in ascending deal order on both
    // sides (user protocol 2026-11-13, see `market_lots`).
    let (groups, unknown) = group_deals(jobs, &rows);
    for id in unknown {
        bot.log(&format!("deal {id} is not mine, skipping"));
        reports.skip(&[id]);
    }
    let mut touched = vec![coins];
    for group in &groups {
        let trade = trade_id(group.item);
        touched.push(trade);
        if group.item != trade {
            touched.push(group.item);
        }
    }
    let mut ok = true;
    // Nearest bank in, Varrock west out: the lots of every group at once.
    let secured = secure_lots(bot, &groups, coins)?;
    let Some(flags) = secured else {
        for group in &groups {
            reports.report(bot, group, "failed", "timeout");
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
            reports.report(bot, group, "failed", "short");
            ok = false;
            continue;
        }
        // A late pose arrives with its tick deadline half burnt: past it,
        // the deal still goes as long as the daemon keeps it open (it is
        // the arbiter), up to a hard cap. A partner failure closes it
        // before the deadline too (finding 5).
        match wait_departure(bot, ids, depart, deadline)? {
            DealWatch::Alive => {}
            DealWatch::Closed => {
                bot.log(&format!("deals {ids:?}: closed by the market before the walk"));
                reports.skip(ids);
                ok = false;
                continue;
            }
            DealWatch::GaveUp => {
                reports.report(bot, group, "failed", "timeout");
                ok = false;
                continue;
            }
        }
        let (x, z) = group.tile;
        if bot.travel_to(lifescape_sdk::plan::Tile { x, z, level: 0 }).is_err() {
            bot.log(&format!("deals {ids:?}: square unreachable"));
            reports.report(bot, group, "failed", "timeout");
            ok = false;
            continue;
        }
        for id in ids {
            let _ = market_call("deal_arrived", vec![("account", str_value(&bot.username)), ("deal", Value::Int(*id))]);
        }
        if !meet_and_trade(bot, group, deadline, coins, reports)? {
            ok = false;
        }
    }
    redeposit(bot, &touched);
    let (level, x, z) = start;
    let _ = bot.travel_to(lifescape_sdk::plan::Tile { x, z, level });
    Ok(if ok { 0 } else { 1 })
}

/// Meets the partner on the tile and trades one group. A window that
/// closes before the lots were verified (re-knock or walk click across
/// a fresh window, deals 1116/1124) is nobody's fault: the pair meets
/// again, at most `MEET_RETRIES` times, while the daemon keeps the deal
/// open. True when the group settled.
fn meet_and_trade(bot: &mut Bot, group: &DealGroup, deadline: u64, coins: u16, reports: &mut DeliverReports) -> Result<bool, ScriptError> {
    let ids = &group.ids;
    let trade = trade_id(group.item);
    let mut trading = false;
    let mut retries = 0u32;
    loop {
        match meet_partner(bot, group, deadline)? {
            DealWatch::Alive => {}
            DealWatch::Closed => {
                bot.log(&format!("deals {ids:?}: closed by the market while waiting"));
                reports.skip(ids);
                return Ok(false);
            }
            DealWatch::GaveUp => {
                bot.log(&format!("deals {ids:?}: partner absent"));
                reports.report(bot, group, "failed", if trading { "timeout" } else { "no_show" });
                return Ok(false);
            }
        }
        if !trading {
            reports.report(bot, group, "trading", "");
            trading = true;
        }
        // Seller lots: the carried form. Buyer lots: whatever the seller
        // put up, validated as the deal item at the exact summed quantity.
        let (give, want) = if group.buying {
            match seller_offer(bot, group, trade, deadline)? {
                SellerOffer::Complete(lots) => (vec![(coins, group.coins)], lots),
                SellerOffer::Closed if retries < MEET_RETRIES => {
                    retries += 1;
                    continue;
                }
                SellerOffer::Closed => {
                    reports.report(bot, group, "failed", "timeout");
                    return Ok(false);
                }
                SellerOffer::Wrong => {
                    let _ = bot.trade_close();
                    reports.report(bot, group, "failed", "mismatch");
                    return Ok(false);
                }
            }
        } else {
            let form = if pack_total(bot, group.item) >= group.qty { group.item } else { trade };
            if pack_total(bot, form) < group.qty {
                bot.log(&format!("deals {ids:?}: lots gone"));
                let _ = bot.trade_close();
                reports.report(bot, group, "failed", "short");
                return Ok(false);
            }
            (vec![(form, group.qty)], vec![(coins, group.coins)])
        };
        let left = (deadline + WAIT_PAST_DEADLINE_TICKS).saturating_sub(bot.world.ticks).max(1);
        let (outcome, reason) = match bot.trade_exchange(&group.partner, &give, 0, &want, 0, left.max(600).min(3000))? {
            ExchangeOutcome::Settled => {
                bot.log(&format!("deals {ids:?} settled ({} x{} for {} coins)", group.item, group.qty, group.coins));
                ("settled", "")
            }
            ExchangeOutcome::WindowClosed if retries < MEET_RETRIES => {
                bot.log(&format!("deals {ids:?}: window closed before the lots, meeting again"));
                retries += 1;
                continue;
            }
            ExchangeOutcome::PartnerMismatch | ExchangeOutcome::ConfirmMismatch => {
                bot.log(&format!("deals {ids:?}: offer mismatch, refused"));
                ("failed", "mismatch")
            }
            ExchangeOutcome::Declined => {
                // Window closed without "Accepted trade.": nothing moved,
                // never a settlement (finding 1).
                bot.log(&format!("deals {ids:?}: partner declined or left the confirm screen"));
                ("failed", "timeout")
            }
            other => {
                bot.log(&format!("deals {ids:?} ended: {other:?}"));
                ("failed", "timeout")
            }
        };
        reports.report(bot, group, outcome, reason);
        return Ok(outcome == "settled");
    }
}

/// Usage errors surface before any report: nothing was posed to us.
#[cfg(test)]
mod tests {
    use crate::market_handover::parse_deliver_args;

    #[test]
    fn deliver_args_parse_or_refuse() {
        let args: Vec<String> = ["deliver", "10", "900", "7=3212,3430"].iter().map(|text| text.to_string()).collect();
        let (depart, deadline, jobs) = parse_deliver_args(&args).expect("parses");
        assert_eq!((depart, deadline, jobs), (10, 900, vec![(7, 3212, 3430)]));
        assert!(parse_deliver_args(&args[..3]).is_err(), "no deal token");
    }
}
