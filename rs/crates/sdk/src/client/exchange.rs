//! Physical exchange: banknotes plus the double-screen trade.
//!
//! `trade_exchange` offers our lots, requires the partner's main-screen
//! offer to match exactly, then requires the confirm screen to show the
//! same lots before the final accept. Any deviation closes the window
//! instead of accepting. Settled only on the server's "Accepted trade.";
//! a confirm window that closes without it is `Declined`. `trade_swap`
//! is a thin caller over this.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::client::Bot;
use crate::error::Result;

/// Typed end of a physical exchange. Transport and operator stops still
/// propagate as `Err`; every game-level refusal is a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExchangeOutcome {
    /// Both screens verified, both accepts through, window closed.
    Settled,
    /// No window opened with the partner before the bound.
    NoWindow,
    /// We do not carry the full give lots.
    Short,
    /// One of our lots is untradeable (server refusal).
    Untradeable,
    /// The partner's main-screen offer differs from the want lots.
    PartnerMismatch,
    /// The main-screen window closed before both offers were verified:
    /// nothing moved and nobody is wrong (a re-knock or a walk click
    /// crossing the freshly opened window closes it, "Other player
    /// declined trade."). The caller may meet again.
    WindowClosed,
    /// The confirm screen differs from the agreed lots.
    ConfirmMismatch,
    /// The confirm window closed without the server's "Accepted trade.":
    /// the partner declined, timed out or logged out. Nothing moved
    /// (`[if_close,tradeconfirm]` returns both offers to their owners).
    Declined,
}

impl Bot {
    /// Bank-note mode switch (`bank_main:com_93` = notes, `com_94` = items).
    /// The engine resets it on every bank open, so every noted withdrawal
    /// sets it explicitly instead of assuming it stuck.
    pub fn bank_set_noted(&mut self, noted: bool) -> Result<()> {
        self.act(format!("bank_set_noted {noted}"));
        let com = (self.tables.com_id)("bank_main:com_93").unwrap_or(5386);
        let off = (self.tables.com_id)("bank_main:com_94").unwrap_or(5387);
        self.if_button(if noted { com } else { off })?;
        self.pump(2)?;
        Ok(())
    }

    /// Withdraw the bank stack of `base` as its banknote `note` (caller
    /// resolves the `cert_` id via gamedata; coins never need this). The
    /// bank holds the base object only: the slot is the base, the pack
    /// receives the note. Restores item mode after, so a later plain
    /// withdrawal never surprises the caller. Returns the notes carried.
    pub fn bank_withdraw_noted(&mut self, base: u16, note: u16, count: u32, timeout: u64) -> Result<u32> {
        self.act(format!("bank_withdraw_noted {} x{count}", (self.tables.obj_name)(base).unwrap_or_default()));
        self.bank_set_noted(true)?;
        let carried = self.withdraw_as(base, note, count, timeout);
        self.bank_set_noted(false)?;
        carried
    }

    /// Exact multiset match of an offer window against expected lots
    /// (zero-count lots ignored). Pure so the double-screen check the
    /// exchange runs is locked by test, not by live luck.
    pub fn lots_match(offered: &std::collections::HashMap<u16, u32>, lots: &[(u16, u32)]) -> bool {
        let mut wanted = std::collections::HashMap::new();
        for (obj, count) in lots {
            if *count > 0 {
                wanted.insert(*obj, *count);
            }
        }
        if offered.len() != wanted.len() {
            return false;
        }
        wanted.iter().all(|(obj, count)| offered.get(obj).copied().unwrap_or(0) == *count)
    }

    /// Coins object id for the separate coin legs of an exchange.
    fn coins_obj(&self) -> u16 {
        (self.tables.find_obj_by_name)("coins").unwrap_or(995)
    }

    /// Full lots (items plus the coin leg) for one side of an exchange.
    fn with_coins(&self, lots: &[(u16, u32)], coins: u32) -> Vec<(u16, u32)> {
        let mut full = lots.to_vec();
        if coins > 0 {
            full.push((self.coins_obj(), coins));
        }
        full
    }

    /// One lot onto the trade window: a single all-click when the carried
    /// stack equals the lot (coins, notes), paced clicks otherwise.
    fn offer_lot(&mut self, obj_id: u16, count: u32, timeout: u64) -> Result<bool> {
        // `trade_offer` already clicks Offer-All on the trade pack when the
        // carried stack equals the lot. The old shortcut here compared the
        // lot with `inventory()` (pack plus its trade-side mirror, so never
        // equal) and would have clicked the hidden `inventory:inv`.
        self.trade_offer(obj_id, count, timeout)
    }

    /// Main-screen loop left without verified lots: a closed window is
    /// no mismatch (live 2026-11-13, deals 1116 and 1124: the partner's
    /// re-knock closed the window it had just opened, both sides
    /// reported `mismatch` and the honest one lost its relist).
    pub(crate) fn off_main_verdict(modal: Option<&str>) -> ExchangeOutcome {
        match modal {
            None => ExchangeOutcome::WindowClosed,
            Some(_) => ExchangeOutcome::PartnerMismatch,
        }
    }

    pub(crate) fn offered_of(&self, obj_id: u16) -> u32 {
        self.world
            .inv
            .get(&Self::TRADE_OFFER)
            .map(|slots| slots.values().filter(|(held, _)| *held == obj_id).map(|(_, n)| *n).sum())
            .unwrap_or(0)
    }

    /// True when fresh engine chatter refuses the item ("You can't trade
    /// this item."): the window never fills, fail fast instead of burning
    /// the timeout.
    pub(crate) fn trade_refused(messages: &[String], seen: usize) -> bool {
        messages.get(seen..).unwrap_or(&[]).iter().any(|message| message.to_lowercase().contains("can't trade"))
    }

    /// True when the server confirmed a completed trade at or after
    /// `since` (server tick). Rev-274 `trade.rs2`: both parties get
    /// "Accepted trade." exactly when `both_moveinv` ran; a decline,
    /// timeout or logout closes the same window with "Other player
    /// declined trade." instead. Tick-scoped (not index-scoped) so the
    /// log trim past `MESSAGE_CAP` never hides the confirmation.
    pub fn trade_accepted_in(messages: &[String], message_ticks: &[u64], since: u64) -> bool {
        messages
            .iter()
            .zip(message_ticks.iter())
            .rev()
            .take_while(|(_, tick)| **tick >= since)
            .any(|(message, _)| message.to_lowercase().contains("accepted trade."))
    }

    /// Live form of `trade_accepted_in` on this bot's message log.
    pub fn trade_accepted_since(&self, since: u64) -> bool {
        Self::trade_accepted_in(&self.world.messages, &self.world.message_ticks, since)
    }

    /// The confirm screen's inventories: ours (inv1, inv2), the partner's
    /// (otherinv1, otherinv2) (rev-274 `trade.rs2`).
    fn confirm_coms(&self) -> ([u16; 2], [u16; 2]) {
        (
            [(self.tables.com_id)("tradeconfirm:inv1").unwrap_or(3542), (self.tables.com_id)("tradeconfirm:inv2").unwrap_or(3538)],
            [(self.tables.com_id)("tradeconfirm:otherinv1").unwrap_or(3532), (self.tables.com_id)("tradeconfirm:otherinv2").unwrap_or(3539)],
        )
    }

    /// Forgets the trade screens' last contents (the client keeps a closed
    /// window's inventories): always both confirm sides, the two offers
    /// only when `offers` (no window open yet: an open window's offers are
    /// the fresh ones its opening streamed).
    pub(crate) fn forget_trade_screens(&mut self, offers: bool) {
        let (confirm, other) = self.confirm_coms();
        let mut screens = vec![confirm[0], confirm[1], other[0], other[1]];
        if offers {
            screens.extend([Self::TRADE_OFFER, Self::TRADE_PARTNER_OFFER]);
        }
        for com in screens {
            self.world.inv.remove(&com);
        }
        self.world.touch_inv();
    }

    /// Physical exchange with a double-screen check: offer our give lots
    /// (plus the coin leg), require the partner's main-screen offer to
    /// equal the want lots exactly, accept, then require the confirm
    /// screen to show the same lots on both sides before the final
    /// accept. Any deviation closes the window instead of accepting.
    pub fn trade_exchange(
        &mut self,
        partner: &str,
        give: &[(u16, u32)],
        coins_give: u32,
        want: &[(u16, u32)],
        coins_want: u32,
        timeout: u64,
    ) -> Result<ExchangeOutcome> {
        self.act(format!("trade_exchange with {partner}"));
        self.coin_context(super::coin_ledger::kind::TRADE);
        let full_give = self.with_coins(give, coins_give);
        let full_want = self.with_coins(want, coins_want);
        for (obj, count) in &full_give {
            if self.inventory().get(obj).copied().unwrap_or(0) < *count {
                return Ok(ExchangeOutcome::Short);
            }
        }
        let deadline = self.world.ticks + timeout;
        let seen = self.world.messages.len();
        // A previous trade's screens never judge this one (live 2026-11-19,
        // deal 51993: bellarhodes and hill_eric traded twice in a row, the
        // confirm check read the first trade's lots, "offer mismatch").
        // An open window's offers are this trade's (the knock before this
        // call opened it; the server sends them once, on open): only the
        // confirm screens, sent later, are forgotten then (deal 52454: the
        // fresh offers wiped, the partner's lots never seen).
        self.forget_trade_screens(self.trade_modal().is_none());
        while self.world.ticks < deadline && self.trade_modal().is_none() {
            let left = deadline.saturating_sub(self.world.ticks).min(600).max(1);
            if self.trade_request(partner, left)? {
                break;
            }
        }
        if self.trade_modal().is_none() {
            return Ok(ExchangeOutcome::NoWindow);
        }
        for (obj, count) in &full_give {
            let left = deadline.saturating_sub(self.world.ticks).max(1);
            if !self.offer_lot(*obj, *count, left.min(300))? {
                if self.trade_modal().is_none() {
                    return Ok(ExchangeOutcome::WindowClosed);
                }
                let _ = self.trade_close();
                if Self::trade_refused(&self.world.messages, seen) {
                    return Ok(ExchangeOutcome::Untradeable);
                }
                return Ok(ExchangeOutcome::Short);
            }
        }
        // Main screen: exact on both sides, or refuse. A partner overpay
        // is a mismatch too (never accept a window we did not agree).
        while self.world.ticks < deadline && self.trade_modal() == Some("main") {
            let own: HashMap<u16, u32> =
                self.world.inv.get(&Self::TRADE_OFFER).map(|slots| slots.values().fold(HashMap::new(), |mut acc, (held, n)| {
                    *acc.entry(*held).or_insert(0) += *n;
                    acc
                })).unwrap_or_default();
            if Self::lots_match(&own, &full_give) && Self::lots_match(&self.trade_partner_offer(), &full_want) {
                break;
            }
            if Self::trade_refused(&self.world.messages, seen) {
                let _ = self.trade_close();
                return Ok(ExchangeOutcome::Untradeable);
            }
            self.pump(5)?;
        }
        if self.trade_modal() != Some("main") {
            let verdict = Self::off_main_verdict(self.trade_modal());
            if verdict != ExchangeOutcome::WindowClosed {
                let _ = self.trade_close();
            }
            return Ok(verdict);
        }
        let partner_now = self.trade_partner_offer();
        if !Self::lots_match(&partner_now, &full_want) {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::PartnerMismatch);
        }
        let left = deadline.saturating_sub(self.world.ticks).max(1);
        self.trade_accept(left.min(300))?;
        self.pump(5)?;
        if self.trade_modal() != Some("confirm") {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::PartnerMismatch);
        }
        // Confirm screen: our lots in inv1+inv2, theirs in otherinv1+2
        // (rev-274 `trade.rs2` transmits each side to its own pair).
        let (confirm_coms, other_coms) = self.confirm_coms();
        let aggregate = |bot: &Self, coms: &[u16; 2]| {
            let mut acc = HashMap::new();
            for com in coms {
                if let Some(slots) = bot.world.inv.get(com) {
                    for (held, n) in slots.values() {
                        *acc.entry(*held).or_insert(0) += *n;
                    }
                }
            }
            acc
        };
        // The confirm lots stream a few ticks after the screen: judged once
        // they agree or `CONFIRM_STREAM_TICKS` passed, never on the first
        // frame (an empty or late screen read as a mismatch).
        let streamed_by = self.world.ticks + CONFIRM_STREAM_TICKS;
        let agree = |bot: &Self| Self::lots_match(&aggregate(bot, &confirm_coms), &full_give) && Self::lots_match(&aggregate(bot, &other_coms), &full_want);
        while !agree(self) && self.world.ticks < streamed_by && self.trade_modal() == Some("confirm") {
            self.pump(1)?;
        }
        if !agree(self) {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::ConfirmMismatch);
        }
        // The window closing is NOT the proof: the partner declining,
        // timing out or logging out closes it too. Only the server's
        // "Accepted trade." settles (live 2026-11-13: declined trades
        // were recorded as executions).
        let confirm_tick = self.world.ticks;
        while self.world.ticks < deadline && self.trade_modal().is_some() {
            let left = deadline.saturating_sub(self.world.ticks).max(1);
            self.trade_accept(left.min(300))?;
            self.pump(5)?;
        }
        if self.trade_modal().is_some() {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::ConfirmMismatch);
        }
        // The message rides the same server tick as the close; give the
        // frame a few ticks to land before judging.
        let settle_by = self.world.ticks + 10;
        while !self.trade_accepted_since(confirm_tick) && self.world.ticks < settle_by {
            self.pump(2)?;
        }
        if !self.trade_accepted_since(confirm_tick) {
            return Ok(ExchangeOutcome::Declined);
        }
        Ok(ExchangeOutcome::Settled)
    }
}

/// Ticks the confirm screen's lots get to stream before a mismatch.
const CONFIRM_STREAM_TICKS: u64 = 20;

#[cfg(test)]
mod tests {
    use crate::client::Bot;

/// Deal 51993 (2026-11-19): the first trade of a pair left its screens in
/// the client; the second trade's confirm check read them. A new trade
/// starts with every trade screen forgotten, the pack and bank untouched.
#[test]
fn a_new_trade_forgets_the_last_trade_screens() {
    let mut bot = Bot::new("127.0.0.1", 43594, ".", ".");
    let mut lots = std::collections::HashMap::new();
    lots.insert(0u8, (995u16, 990u32));
    for com in [Bot::TRADE_OFFER, Bot::TRADE_PARTNER_OFFER, 3542, 3538, 3532, 3539, 3322] {
        bot.world.inv.insert(com, lots.clone());
    }
    // Deal 52454: a window already open keeps its fresh offers.
    bot.forget_trade_screens(false);
    for com in [3542, 3538, 3532, 3539] {
        assert!(!bot.world.inv.contains_key(&com), "confirm screen {com} forgotten");
    }
    assert!(bot.world.inv.contains_key(&Bot::TRADE_OFFER) && bot.world.inv.contains_key(&Bot::TRADE_PARTNER_OFFER), "an open window's offers stay");
    bot.forget_trade_screens(true);
    for com in [Bot::TRADE_OFFER, Bot::TRADE_PARTNER_OFFER] {
        assert!(!bot.world.inv.contains_key(&com), "no window yet: offer {com} forgotten");
    }
    assert!(bot.world.inv.contains_key(&3322), "the trade pack stays");
}
/// The double-screen gate is exact: same keys and counts pass, an
/// overpay or a missing lot refuses, zero lots never count.
#[test]
fn lots_match_is_exact_on_both_screens() {
    use std::collections::HashMap;
    let offered: HashMap<u16, u32> = [(10u16, 5u32), (995u16, 100u32)].into_iter().collect();
    assert!(Bot::lots_match(&offered, &[(10, 5), (995, 100)]), "exact passes");
    assert!(Bot::lots_match(&offered, &[(995, 100), (10, 5), (11, 0)]), "zero lot ignored");
    assert!(!Bot::lots_match(&offered, &[(10, 5)]), "missing key refuses");
    assert!(!Bot::lots_match(&offered, &[(10, 5), (995, 100), (12, 1)]), "extra key refuses");
    assert!(!Bot::lots_match(&offered, &[(10, 4), (995, 100)]), "short count refuses");
    assert!(!Bot::lots_match(&offered, &[(10, 6), (995, 100)]), "overpay refuses");
    assert!(Bot::lots_match(&HashMap::new(), &[]), "empty agrees with empty");
    assert!(!Bot::lots_match(&HashMap::new(), &[(10, 1)]), "empty never matches a lot");
}

/// Deals 1116/1124: the main screen closing under us is a closed
/// window (meet again), never a partner mismatch.
#[test]
fn closed_main_screen_is_not_a_mismatch() {
    assert_eq!(Bot::off_main_verdict(None), super::ExchangeOutcome::WindowClosed);
    assert_eq!(Bot::off_main_verdict(Some("confirm")), super::ExchangeOutcome::PartnerMismatch, "moved on without us");
}

/// Refusal scope is the same discipline as the bank fan: fresh
/// server chatter fails fast, older traffic never matches.
#[test]
fn trade_refused_only_matches_fresh_refusal() {
    let messages = vec!["Welcome.".to_string(), "You can't trade this item.".to_string()];
    assert!(!Bot::trade_refused(&messages, 2), "nothing fresh");
    assert!(Bot::trade_refused(&messages, 1), "fresh refusal fails fast");
    let other = vec!["A trade request.".to_string()];
    assert!(!Bot::trade_refused(&other, 0), "unrelated text passes");
}

/// Finding 1 (2026-11-13): a closed confirm window is not a trade. Only
/// the server's "Accepted trade." since the confirm click settles; a
/// decline (or an older acceptance) never does.
#[test]
fn only_a_fresh_acceptance_settles() {
    let text = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect::<Vec<String>>();
    let declined = text(&["Sending trade offer...", "Other player declined trade."]);
    assert!(!Bot::trade_accepted_in(&declined, &[100, 140], 120), "decline never settles");
    let accepted = text(&["Sending trade offer...", "Accepted trade."]);
    assert!(Bot::trade_accepted_in(&accepted, &[100, 141], 120), "acceptance settles");
    assert!(!Bot::trade_accepted_in(&accepted, &[100, 110], 120), "an older acceptance is another trade");
    let waiting = text(&["Other player has accepted."]);
    assert!(!Bot::trade_accepted_in(&waiting, &[130], 120), "partner-side status is not a trade");
    assert!(!Bot::trade_accepted_in(&[], &[], 0), "empty log never settles");
}
}
