//! Handover mechanics for `market deliver` (split from the activity so it
//! stays under its plafond): watching the deal against the daemon,
//! meeting the partner, reading the seller's offer, one terminal report
//! per posed deal (error paths included), redeposit.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use lifescape_sdk::Bot;

use crate::kit::bank::nearest_bank;
use crate::market_client::{deal_report, my_deals, offer_state, trade_id, OfferState};
use crate::market_lots::{Carried, DealGroup};
use crate::ScriptError;

/// How long the buyer watches a partial offer grow before refusing it.
pub const OFFER_WAIT_TICKS: u64 = 120;

/// How long the buyer waits for a first lot in a window opened past the
/// deadline (the seller clicks within a few ticks).
pub const OFFER_OPEN_TICKS: u64 = 300;

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

/// Anchors a convocation on the worker's own clock at reception: the
/// daemon sends `deliver +<depart_in> +<deadline_in> ...` (ticks from
/// now), the worker turns them into ticks of its login epoch. Live
/// 2026-11-13 (deal 1276): the daemon stamped absolute ticks from the
/// snapshot of the PREVIOUS login (61826) on a worker reopened at tick 1,
/// which then waited 61 800 ticks for its departure while the buyer
/// stood at the square. Absolute ticks (operator) pass through.
pub fn anchor_deliver_args(args: &[String], now: u64) -> Vec<String> {
    let mut out = args.to_vec();
    if out.first().map(String::as_str) != Some("deliver") {
        return out;
    }
    for slot in out.iter_mut().skip(1).take(2) {
        if let Some(offset) = slot.strip_prefix('+').and_then(|text| text.parse::<u64>().ok()) {
            *slot = now.saturating_add(offset).to_string();
        }
    }
    out
}

/// Parses `deliver <depart> <deadline> <deal>=<x>,<y> ...`.
pub fn parse_deliver_args(args: &[String]) -> Result<(u64, u64, Vec<(i64, i32, i32)>), ScriptError> {
    if args.len() < 4 {
        return Err(ScriptError::Usage("market deliver <depart> <deadline> <deal>=<x>,<y> ...".to_string()));
    }
    // `+N` offsets are anchored at pose time (`anchor_deliver_args`);
    // one reaching the run was never anchored, and `u64::parse` would
    // read it as an absolute tick of this login.
    if args[1].starts_with('+') || args[2].starts_with('+') {
        return Err(ScriptError::Usage(format!("unanchored convocation offsets {:?} {:?}", args[1], args[2])));
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

/// Times a pair meets again after a window closed before the lots
/// (re-knock race): bounded, the deal watch still releases earlier.
pub const MEET_RETRIES: u32 = 3;

/// The partner is "at the meeting point" within this many tiles
/// (Chebyshev) of our own assigned tile: the convened tiles are
/// adjacent, a nav-adjusted arrival lands a tile or two off.
pub const MEET_RADIUS: i32 = 3;

/// Ticks the partner must hold the same tile before a knock: a partner
/// walking past (to its bank) moves every tick and is never knocked.
pub const PARTNER_STILL_TICKS: u64 = 3;

/// Beyond this many tiles from our own tile, walk back before waiting.
pub const HOME_RADIUS: i32 = 1;

/// Budget of one walk back to the own tile.
pub const RETURN_TICKS: u64 = 120;

/// Cadence of the "cannot see" diagnostic while waiting on the tile.
pub const SEE_LOG_TICKS: u64 = 200;

/// Ticks without any sight of the partner before a later group whose
/// partner already waits at its point is served first.
pub const SERVE_FIRST_AFTER_TICKS: u64 = 100;

/// How one group's turn ended (`market deliver`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// Terminal for this group: settled (`true`) or failed/closed.
    Done(bool),
    /// Index (in the `later` list) of a group whose partner waits at its
    /// point: served first, this group comes back after it.
    ServeFirst(usize),
}

/// Outcome of one meeting wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meet {
    /// The daemon verdict (`Alive`: the window is open).
    Watch(DealWatch),
    /// Index (in `later`) of a group to serve before this one.
    ServeFirst(usize),
}

/// Pure queue rule (live 2026-11-13, deals 1684/1687/1689): bd23dqpq
/// waited on its own tile for an absent bcm9kxn4 (deal 1687) while
/// nina_rhodes stood still at the 1689 point six tiles away; she waited
/// out the hard cap and reported the present, busy bd23dqpq `no_show`.
/// A partner out of sight for `SERVE_FIRST_AFTER_TICKS` yields to the
/// first later group whose partner is ready.
pub fn serve_first(unseen_for: u64, ready: &[bool]) -> Option<usize> {
    if unseen_for < SERVE_FIRST_AFTER_TICKS {
        return None;
    }
    ready.iter().position(|ready| *ready)
}

/// What the waiting side does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetStep {
    /// The partner holds still at the meeting point: one knock.
    Knock,
    /// Drifted off the own tile (a knock's approach or server follow):
    /// walk back, the partner looks for us there.
    Return,
    /// Stay put: the partner is absent, passing by, or elsewhere.
    Wait,
}

fn chebyshev(from: (i32, i32), to: (i32, i32)) -> i32 {
    (from.0 - to.0).abs().max((from.1 - to.1).abs())
}

/// Pure meeting rule: never leave the assigned tile for a partner seen
/// anywhere but at the meeting point (deals 1201/1248/1267/1268: a
/// partner knocked in transit was followed 16-28 tiles away, past the
/// 15-tile view, and the pair never saw each other again).
pub fn meet_step(own_tile: (i32, i32), me: (i32, i32), partner: Option<(i32, i32)>, partner_still: u64) -> MeetStep {
    if let Some(seen) = partner {
        if chebyshev(seen, own_tile) <= MEET_RADIUS && partner_still >= PARTNER_STILL_TICKS {
            return MeetStep::Knock;
        }
    }
    if chebyshev(me, own_tile) > HOME_RADIUS {
        return MeetStep::Return;
    }
    MeetStep::Wait
}

/// How long the partner has held the same tile, from the first sighting
/// there; a move or a disappearance restarts the count.
#[derive(Debug, Default)]
pub struct PartnerStill {
    last: Option<((i32, i32), u64)>,
}

impl PartnerStill {
    pub fn observe(&mut self, seen: Option<(i32, i32)>, now: u64) -> u64 {
        match (seen, self.last) {
            (Some(tile), Some((known, since))) if tile == known => now.saturating_sub(since),
            (Some(tile), _) => {
                self.last = Some((tile, now));
                0
            }
            (None, _) => {
                self.last = None;
                0
            }
        }
    }
}

/// The partner's tile when it is in view on our level.
fn partner_tile(bot: &Bot, partner: &str) -> Option<(i32, i32)> {
    let pid = bot.find_player(partner)?;
    let player = bot.world.players.get(&pid)?;
    (player.level == bot.world.level).then_some((player.x, player.z))
}

/// Mutual knocks open the window (one-sided requests sit as "Sending..."
/// until the other side knocks back). Waits on the own tile for a late
/// partner while the deal lives, checked every `DEAL_WATCH_TICKS`
/// whatever the deadline, never past the hard cap; knocks only a partner
/// holding still at the meeting point (`meet_step`). `Watch(Alive)` means
/// the window is open. While this partner stays out of sight, a partner
/// of a `later` group already waiting at its own point is served first
/// (`serve_first`).
pub fn meet_partner(bot: &mut Bot, group: &DealGroup, deadline: u64, later: &[&DealGroup]) -> Result<Meet, ScriptError> {
    let mut last_check = bot.world.ticks;
    let mut last_log: Option<u64> = None;
    let mut still = PartnerStill::default();
    let mut later_still: Vec<PartnerStill> = later.iter().map(|_| PartnerStill::default()).collect();
    let mut last_seen = bot.world.ticks;
    while bot.trade_modal().is_none() {
        if bot.world.ticks.saturating_sub(last_check) >= DEAL_WATCH_TICKS {
            last_check = bot.world.ticks;
            let watch = deal_watch(bot, &group.ids, deadline);
            if watch != DealWatch::Alive {
                return Ok(Meet::Watch(watch));
            }
        }
        if bot.world.ticks >= deadline + WAIT_PAST_DEADLINE_TICKS {
            return Ok(Meet::Watch(DealWatch::GaveUp));
        }
        let seen = partner_tile(bot, &group.partner);
        if seen.is_some() {
            last_seen = bot.world.ticks;
        }
        let ready: Vec<bool> = later
            .iter()
            .zip(later_still.iter_mut())
            .map(|(other, watch)| {
                let spot = partner_tile(bot, &other.partner);
                let held = watch.observe(spot, bot.world.ticks);
                spot.is_some_and(|tile| chebyshev(tile, other.tile) <= MEET_RADIUS) && held >= PARTNER_STILL_TICKS
            })
            .collect();
        if let Some(index) = serve_first(bot.world.ticks.saturating_sub(last_seen), &ready) {
            bot.log(&format!(
                "deals {:?}: {} out of sight, serving {} first (deals {:?}, waiting at its point)",
                group.ids, group.partner, later[index].partner, later[index].ids
            ));
            return Ok(Meet::ServeFirst(index));
        }
        let held = still.observe(seen, bot.world.ticks);
        let (_, my_x, my_z) = bot.position();
        let step = meet_step(group.tile, (my_x, my_z), seen, held);
        if step != MeetStep::Knock && last_log.is_none_or(|tick| bot.world.ticks.saturating_sub(tick) >= SEE_LOG_TICKS) {
            last_log = Some(bot.world.ticks);
            // Slot, position and name of every tracked player: tells an
            // empty view from a nameless one or a partner seen elsewhere.
            bot.log(&format!(
                "deals {:?}: waiting for {} at {:?} from ({my_x},{my_z}), seen {seen:?} ({} players tracked: {:?})",
                group.ids,
                group.partner,
                group.tile,
                bot.world.players.len(),
                bot.debug_players()
            ));
        }
        match step {
            MeetStep::Knock => {
                // One knock (one quiet wait): the partner is at the point,
                // the knock's approach stays within a few tiles of it.
                if bot.trade_request(&group.partner, 1)? {
                    break;
                }
            }
            MeetStep::Return => {
                let (x, z) = group.tile;
                match bot.walk_to(x, z, 0, RETURN_TICKS) {
                    Ok(()) => {}
                    Err(error) if lifescape_sdk::error::is_interrupted(&error) => return Err(error.into()),
                    Err(error) => {
                        bot.log(&format!("deals {:?}: walk back to {:?} failed: {error}", group.ids, group.tile));
                        bot.pump(5)?;
                    }
                }
            }
            MeetStep::Wait => {
                bot.pump(5)?;
            }
        }
    }
    Ok(Meet::Watch(if bot.trade_modal().is_some() { DealWatch::Alive } else { DealWatch::GaveUp }))
}

/// What the buyer read from the seller's side of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SellerOffer {
    /// The exact deal lots, in the forms the seller put up.
    Complete(Vec<(u16, u32)>),
    /// Another object, coins, or more than the deal: a real mismatch.
    Wrong,
    /// The window closed before the offer was complete: nothing moved,
    /// nobody is wrong, the parties may meet again (deal 1116).
    Closed,
}

/// Pure verdict once the wait ended: a window that closed under a
/// partial (or empty) offer is no mismatch.
pub fn offer_verdict(state: OfferState, window_open: bool) -> SellerOffer {
    match state {
        OfferState::Complete(lots) => SellerOffer::Complete(lots),
        OfferState::Partial if !window_open => SellerOffer::Closed,
        _ => SellerOffer::Wrong,
    }
}

/// Buyer side: the seller clicks its lots in by steps; wait for the whole
/// offer (bounded: the deadline, or `OFFER_OPEN_TICKS` for a window that
/// opened past it — a late pair still trades while the daemon keeps the
/// deal open).
pub fn seller_offer(bot: &mut Bot, group: &DealGroup, trade: u16, deadline: u64) -> Result<SellerOffer, ScriptError> {
    let until = deadline.max(bot.world.ticks + OFFER_OPEN_TICKS);
    while bot.world.ticks < until && bot.trade_modal() == Some("main") && bot.trade_partner_offer().is_empty() {
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
    let verdict = offer_verdict(state, bot.trade_modal().is_some());
    match &verdict {
        SellerOffer::Complete(_) => {}
        SellerOffer::Closed => bot.log(&format!("deals {:?}: window closed before the offer ({observed:?})", group.ids)),
        SellerOffer::Wrong => bot.log(&format!(
            "deals {:?}: offer mismatch ({observed:?} for {} x{}), refused",
            group.ids, group.item, group.qty
        )),
    }
    Ok(verdict)
}

/// Pack ids to redeposit: everything carried except the untouched.
/// The touched set must hold every dealt form (base AND note): the
/// seller's window form may differ from the trade id, and a form kept
/// by mistake never returns to the bank.
pub fn redeposit_keep(carried: &[u16], touched: &[u16]) -> Vec<u16> {
    carried.iter().copied().filter(|held| !touched.contains(held)).collect()
}

/// Best-effort redeposit of the traded stacks (notes land back as base
/// stock), then the paused script's own pack comes back out (`restore`,
/// `market_lots::carried_before`): coins a shop buy was carrying, the
/// load a woodcutter was walking to the bank. Locked units stay banked
/// (outside the trade procedure). Loud when it fails: a silent drop here
/// loses the lots with no trace.
pub fn redeposit(bot: &mut Bot, touched: &[u16], restore: &[Carried]) {
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
    for stack in restore {
        let note = trade_id(stack.base);
        let form = if stack.noted && note != stack.base { note } else { stack.base };
        // Only the missing part: a trip that never banked (nearest bank
        // shut) still carries its pack, never withdraw it twice.
        let missing = stack.units.saturating_sub(crate::kit::pack::pack_total(bot, form));
        if missing == 0 {
            continue;
        }
        let outcome = if form != stack.base {
            bot.bank_withdraw_noted(stack.base, note, missing, 300)
        } else {
            bot.bank_withdraw(stack.base, missing, 300)
        };
        let outcome = outcome.map(|_| crate::kit::pack::pack_total(bot, form));
        match outcome {
            Ok(held) if held >= stack.units => {}
            other => bot.log(&format!("restore of the paused pack: obj {} x{} ({}) came back as {other:?}", stack.base, stack.units, if stack.noted { "noted" } else { "base" })),
        }
    }
    let _ = bot.close_modal();
}

#[cfg(test)]
mod tests {
    use super::{anchor_deliver_args, error_verdict, offer_verdict, SellerOffer, redeposit_keep, watch_verdict, DealWatch, WAIT_PAST_DEADLINE_TICKS};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    /// Deal 1116: ryan_park's window closed under an empty offer when
    /// frank_lane re-knocked; that is a closed window, not a mismatch.
    #[test]
    fn closed_window_is_not_a_mismatch() {
        use crate::market_client::OfferState;
        assert_eq!(offer_verdict(OfferState::Partial, false), SellerOffer::Closed, "empty offer, window gone");
        assert_eq!(offer_verdict(OfferState::Partial, true), SellerOffer::Wrong, "window open, offer never completed");
        assert_eq!(offer_verdict(OfferState::Wrong, false), SellerOffer::Wrong, "a wrong lot stays wrong");
        assert_eq!(offer_verdict(OfferState::Complete(vec![(841, 1)]), true), SellerOffer::Complete(vec![(841, 1)]));
    }

    /// Deal 1276: the convocation lands on a worker reopened at tick 1.
    /// Relative ticks anchor on the worker's clock whatever epoch the
    /// daemon's snapshot came from; absolute ones pass through.
    #[test]
    fn convocation_anchors_on_the_worker_clock() {
        let posed = strings(&["deliver", "+30", "+1652", "1276=3209,3425"]);
        assert_eq!(anchor_deliver_args(&posed, 1), strings(&["deliver", "31", "1653", "1276=3209,3425"]));
        assert_eq!(anchor_deliver_args(&posed, 59_000), strings(&["deliver", "59030", "60652", "1276=3209,3425"]));
        let absolute = strings(&["deliver", "61826", "63478", "1276=3209,3425"]);
        assert_eq!(anchor_deliver_args(&absolute, 1), absolute, "operator ticks untouched");
        let other = strings(&["flip", "+30"]);
        assert_eq!(anchor_deliver_args(&other, 1), other, "only deliver convocations");
        assert!(super::parse_deliver_args(&posed).is_err(), "an unanchored offset never reads as a tick");
    }

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

    /// Deals 1201/1248/1267/1268: the pair stood on adjacent assigned
    /// tiles, but one side had knocked a partner it saw walking past (to
    /// its bank) and followed it 16 to 28 tiles away (server follow of
    /// op_player + approach), then never came back: out of the 15-tile
    /// view, both waited 4 minutes for an invisible partner. Knock only
    /// a partner holding still at the meeting point; otherwise wait on
    /// (or walk back to) the own tile.
    #[test]
    fn meeting_stays_on_the_assigned_tile() {
        use super::{meet_step, MeetStep, PARTNER_STILL_TICKS};
        let own = (3215, 3425);
        let still = PARTNER_STILL_TICKS;
        assert_eq!(meet_step(own, own, Some((3230, 3428)), still), MeetStep::Wait, "1268: partner 15 tiles off, never chased");
        assert_eq!(meet_step(own, own, Some((3214, 3425)), 0), MeetStep::Wait, "partner passing by the tile, still moving");
        assert_eq!(meet_step(own, own, Some((3216, 3426)), still), MeetStep::Knock, "partner on its adjacent tile");
        assert_eq!(meet_step(own, (3188, 3429), None, 0), MeetStep::Return, "1268: drifted to the bank, partner out of view");
        assert_eq!(meet_step(own, (3188, 3429), Some((3200, 3430)), still), MeetStep::Return, "partner idle far from the point");
        assert_eq!(meet_step(own, (3217, 3426), Some((3216, 3426)), still), MeetStep::Knock, "knock drift around the point");
        assert_eq!(meet_step(own, own, None, 0), MeetStep::Wait, "partner not there yet");
    }

    /// Deal 1689: nina_rhodes waited still at her point while bd23dqpq
    /// waited for an absent partner: past the unseen window, the ready
    /// later group goes first; a partner merely late, or no ready group,
    /// keeps the order.
    #[test]
    fn an_absent_partner_yields_to_a_ready_one() {
        use super::{serve_first, SERVE_FIRST_AFTER_TICKS};
        assert_eq!(serve_first(SERVE_FIRST_AFTER_TICKS, &[false, true]), Some(1));
        assert_eq!(serve_first(SERVE_FIRST_AFTER_TICKS - 1, &[true]), None, "partner seen recently");
        assert_eq!(serve_first(5000, &[false, false]), None, "nobody else ready");
        assert_eq!(serve_first(5000, &[]), None, "last group");
    }

    /// Stillness counts from the first sighting at the same tile; a move
    /// or a disappearance restarts it.
    #[test]
    fn partner_stillness_restarts_on_move() {
        use super::PartnerStill;
        let mut watch = PartnerStill::default();
        assert_eq!(watch.observe(Some((3216, 3426)), 100), 0, "first sighting");
        assert_eq!(watch.observe(Some((3216, 3426)), 105), 5);
        assert_eq!(watch.observe(Some((3216, 3427)), 106), 0, "moved");
        assert_eq!(watch.observe(None, 110), 0, "out of view");
        assert_eq!(watch.observe(Some((3216, 3427)), 111), 0, "back in view, counts again");
        assert_eq!(watch.observe(Some((3216, 3427)), 120), 9);
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
