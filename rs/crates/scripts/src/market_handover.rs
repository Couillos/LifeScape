//! Handover mechanics for `market deliver` (split from the activity so it
//! stays under its plafond): watching the deal against the daemon,
//! meeting the partner, reading the seller's offer, one terminal report
//! per posed deal (error paths included), redeposit.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_sdk::Bot;

use crate::kit::bank::nearest_bank;
use crate::market_client::{deal_report, my_deals, offer_state, OfferState};
use crate::market_lots::DealGroup;
use crate::ScriptError;

/// How long the buyer watches a partial offer grow before refusing it.
pub const OFFER_WAIT_TICKS: u64 = 120;

/// Past its tick deadline a party keeps waiting for a late partner while
/// the daemon keeps the deal open, at most this many ticks more.
pub const WAIT_PAST_DEADLINE_TICKS: u64 = 3000;

/// Cadence of the daemon check while waiting (departure, partner): a
/// deal the partner already failed (short, mismatch) releases this side
/// within this many ticks instead of idling to the deadline (finding 5).
pub const DEAL_WATCH_TICKS: u64 = 100;

/// Where a posed deal group stands while this side waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DealWatch {
    /// The daemon still lists one of the deals open: keep going.
    Alive,
    /// The daemon closed every deal (partner report, sweep): leave
    /// without reporting, the verdict is already written.
    Closed,
    /// Hard cap past the deadline, or the market did not answer: this
    /// side gives up and reports `timeout`.
    GaveUp,
}

/// Pure verdict: `listed` is `Some(any id still open)` from the daemon,
/// `None` when the market did not answer.
pub fn watch_verdict(now: u64, deadline: u64, listed: Option<bool>) -> DealWatch {
    if now >= deadline + WAIT_PAST_DEADLINE_TICKS {
        return DealWatch::GaveUp;
    }
    match listed {
        Some(true) => DealWatch::Alive,
        Some(false) => DealWatch::Closed,
        None => DealWatch::GaveUp,
    }
}

/// Live form of `watch_verdict` against the daemon.
pub fn deal_watch(bot: &Bot, ids: &[i64], deadline: u64) -> DealWatch {
    if bot.world.ticks >= deadline + WAIT_PAST_DEADLINE_TICKS {
        return DealWatch::GaveUp;
    }
    let listed = my_deals(&bot.username).ok().map(|rows| rows.iter().any(|row| ids.contains(&row.id)));
    watch_verdict(bot.world.ticks, deadline, listed)
}

/// Verdict for a deal a run leaves on an error (transport, crash, stop):
/// a deal already in `trading` settled only if the server said
/// "Accepted trade." since the window opened (the goods moved); every
/// other deal fails `timeout` (neutral, both sides relist). Without this
/// the daemon kept the deal in `trading` forever (finding 2).
pub fn error_verdict(trading_since: Option<u64>, accepted_since: &dyn Fn(u64) -> bool) -> (&'static str, &'static str) {
    match trading_since {
        Some(tick) if accepted_since(tick) => ("settled", ""),
        _ => ("failed", "timeout"),
    }
}

/// Reports sent by one delivery run: every posed deal ends with exactly
/// one terminal report, from the normal path or from the error path.
pub struct DeliverReports {
    account: String,
    finished: HashSet<i64>,
    trading_since: HashMap<i64, u64>,
}

impl DeliverReports {
    pub fn new(account: &str) -> Self {
        DeliverReports { account: account.to_string(), finished: HashSet::new(), trading_since: HashMap::new() }
    }

    pub fn report(&mut self, bot: &Bot, group: &DealGroup, outcome: &str, reason: &str) {
        for id in &group.ids {
            deal_report(&self.account, *id, outcome, reason);
            if outcome == "trading" {
                self.trading_since.insert(*id, bot.world.ticks);
            } else {
                self.finished.insert(*id);
            }
        }
    }

    /// Nothing to report for these ids: the daemon already closed them,
    /// or they are not this bot's.
    pub fn skip(&mut self, ids: &[i64]) {
        self.finished.extend(ids.iter().copied());
    }

    /// Error path: one terminal report per deal still open on our side.
    pub fn flush_unfinished(&mut self, bot: &Bot, posed: &[i64]) {
        for id in posed {
            if self.finished.contains(id) {
                continue;
            }
            let since = self.trading_since.get(id).copied();
            let (outcome, reason) = error_verdict(since, &|tick| bot.trade_accepted_since(tick));
            bot.log(&format!("deal {id}: run ended early, reporting {outcome} {reason}"));
            deal_report(&self.account, *id, outcome, reason);
            self.finished.insert(*id);
        }
    }
}

/// Parses `deliver <depart> <deadline> <deal>=<x>,<y> ...`.
pub fn parse_deliver_args(args: &[String]) -> Result<(u64, u64, Vec<(i64, i32, i32)>), ScriptError> {
    if args.len() < 4 {
        return Err(ScriptError::Usage("market deliver <depart> <deadline> <deal>=<x>,<y> ...".to_string()));
    }
    let depart: u64 = args[1].parse().map_err(|_| ScriptError::Usage("depart must be a tick".to_string()))?;
    let deadline: u64 = args[2].parse().map_err(|_| ScriptError::Usage("deadline must be a tick".to_string()))?;
    let mut jobs: Vec<(i64, i32, i32)> = Vec::new();
    for token in &args[3..] {
        let (id, tile) = token.split_once('=').ok_or_else(|| ScriptError::Usage(format!("bad deal token {token:?}")))?;
        let (x, z) = tile.split_once(',').ok_or_else(|| ScriptError::Usage(format!("bad tile {tile:?}")))?;
        jobs.push((
            id.parse().map_err(|_| ScriptError::Usage(format!("bad deal {id:?}")))?,
            x.parse().map_err(|_| ScriptError::Usage(format!("bad x {x:?}")))?,
            z.parse().map_err(|_| ScriptError::Usage(format!("bad z {z:?}")))?,
        ));
    }
    Ok((depart, deadline, jobs))
}

/// Waits for the staggered departure while watching the daemon: a
/// partner that already failed the deal releases us now.
pub fn wait_departure(bot: &mut Bot, ids: &[i64], depart: u64, deadline: u64) -> Result<DealWatch, ScriptError> {
    let mut watch = deal_watch(bot, ids, deadline);
    let mut last_check = bot.world.ticks;
    while watch == DealWatch::Alive && bot.world.ticks < depart && bot.world.ticks < deadline {
        bot.pump(5)?;
        if bot.world.ticks.saturating_sub(last_check) >= DEAL_WATCH_TICKS {
            last_check = bot.world.ticks;
            watch = deal_watch(bot, ids, deadline);
        }
    }
    Ok(if watch == DealWatch::Alive { deal_watch(bot, ids, deadline) } else { watch })
}

/// Mutual knocks open the window (one-sided requests sit as "Sending..."
/// until the other side knocks back). Waits for a late partner while the
/// deal lives, checked every `DEAL_WATCH_TICKS` whatever the deadline,
/// never past the hard cap. `Alive` means the window is open.
pub fn meet_partner(bot: &mut Bot, group: &DealGroup, deadline: u64) -> Result<DealWatch, ScriptError> {
    let mut knocks = 0u32;
    let mut last_check = bot.world.ticks;
    while bot.trade_modal().is_none() {
        if bot.world.ticks.saturating_sub(last_check) >= DEAL_WATCH_TICKS {
            last_check = bot.world.ticks;
            let watch = deal_watch(bot, &group.ids, deadline);
            if watch != DealWatch::Alive {
                return Ok(watch);
            }
        }
        if bot.world.ticks >= deadline + WAIT_PAST_DEADLINE_TICKS {
            return Ok(DealWatch::GaveUp);
        }
        let left = (deadline + WAIT_PAST_DEADLINE_TICKS).saturating_sub(bot.world.ticks).min(DEAL_WATCH_TICKS).max(1);
        if bot.find_player(&group.partner).is_none() && knocks % 20 == 0 {
            bot.log(&format!(
                "deals {:?}: cannot see {} ({} players tracked)",
                group.ids,
                group.partner,
                bot.world.players.len()
            ));
        }
        knocks += 1;
        if bot.trade_request(&group.partner, left)? {
            break;
        }
    }
    Ok(if bot.trade_modal().is_some() { DealWatch::Alive } else { DealWatch::GaveUp })
}

/// Buyer side: the seller clicks its lots in by steps; wait for the whole
/// offer (bounded), `None` on a wrong one.
pub fn seller_offer(bot: &mut Bot, group: &DealGroup, trade: u16, deadline: u64) -> Result<Option<Vec<(u16, u32)>>, ScriptError> {
    while bot.world.ticks < deadline && bot.trade_modal() == Some("main") && bot.trade_partner_offer().is_empty() {
        bot.pump(5)?;
    }
    let settle_by = bot.world.ticks + OFFER_WAIT_TICKS;
    let mut observed = bot.trade_partner_offer();
    let mut state = offer_state(&observed, group.item, trade, group.qty);
    while state == OfferState::Partial && bot.world.ticks < settle_by && bot.trade_modal() == Some("main") {
        bot.pump(3)?;
        observed = bot.trade_partner_offer();
        state = offer_state(&observed, group.item, trade, group.qty);
    }
    if let OfferState::Complete(lots) = state {
        return Ok(Some(lots));
    }
    bot.log(&format!("deals {:?}: offer mismatch ({observed:?} for {} x{}), refused", group.ids, group.item, group.qty));
    Ok(None)
}

/// Pack ids to redeposit: everything carried except the untouched.
/// The touched set must hold every dealt form (base AND note): the
/// seller's window form may differ from the trade id, and a form kept
/// by mistake never returns to the bank.
pub fn redeposit_keep(carried: &[u16], touched: &[u16]) -> Vec<u16> {
    carried.iter().copied().filter(|held| !touched.contains(held)).collect()
}

/// Best-effort redeposit of the traded stacks (notes land back as base
/// stock). Loud when it fails: a silent drop here loses the lots with
/// no trace.
pub fn redeposit(bot: &mut Bot, touched: &[u16]) {
    let Ok(bank) = nearest_bank(bot) else { return };
    if !bot.open_bank(bank.x as i32, bank.z as i32, bank.loc_id as u16, 300).unwrap_or(false) {
        bot.log("redeposit skipped: bank did not open");
        return;
    }
    if let Some(side_com) = (bot.tables.com_id)("bank_side:inv") {
        let carried: Vec<u16> = bot.backpack().1.iter().map(|(_, (held, _))| *held).collect();
        let keep: Vec<u16> = redeposit_keep(&carried, touched);
        match bot.bank_deposit_all(side_com, &keep, 300) {
            Ok(true) => {}
            other => bot.log(&format!("redeposit outcome: {other:?}")),
        }
    }
    let _ = bot.close_modal();
}

#[cfg(test)]
mod tests {
    use super::{error_verdict, redeposit_keep, watch_verdict, DealWatch, WAIT_PAST_DEADLINE_TICKS};

    /// Deal 24 regression: the seller delivered base willow (1519) while
    /// the trade id is the note (1520) — both forms must redeposit.
    #[test]
    fn redeposit_covers_both_dealt_forms() {
        let carried = [995u16, 1519, 314];
        let touched = [995u16, 1520, 1519];
        assert_eq!(redeposit_keep(&carried, &touched), vec![314u16]);
    }

    /// Finding 5: the daemon verdict counts before the deadline too. A
    /// deal the partner already failed releases this side at once; the
    /// hard cap and a silent market give up loudly.
    #[test]
    fn watch_releases_before_the_deadline() {
        assert_eq!(watch_verdict(100, 5000, Some(false)), DealWatch::Closed, "partner failed it early");
        assert_eq!(watch_verdict(100, 5000, Some(true)), DealWatch::Alive);
        assert_eq!(watch_verdict(6000, 5000, Some(true)), DealWatch::Alive, "late partner, deal still open");
        assert_eq!(watch_verdict(5000 + WAIT_PAST_DEADLINE_TICKS, 5000, Some(true)), DealWatch::GaveUp, "hard cap");
        assert_eq!(watch_verdict(100, 5000, None), DealWatch::GaveUp, "market silent");
    }

    /// Finding 2: an aborted run still ends every deal. A deal in
    /// `trading` settles only on the server's acceptance since the
    /// window opened; everything else fails `timeout`.
    #[test]
    fn aborted_run_reports_every_deal() {
        assert_eq!(error_verdict(None, &|_| true), ("failed", "timeout"), "never traded");
        assert_eq!(error_verdict(Some(40), &|since| since <= 40), ("settled", ""), "goods moved before the crash");
        assert_eq!(error_verdict(Some(40), &|_| false), ("failed", "timeout"), "no acceptance, nothing moved");
    }
}
