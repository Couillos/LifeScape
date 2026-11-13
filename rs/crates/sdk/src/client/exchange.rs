//! Physical exchange: banknotes plus the double-screen trade.
//!
//! `trade_exchange` offers our lots, requires the partner's main-screen
//! offer to match exactly, then requires the confirm screen to show the
//! same lots before the final accept. Any deviation closes the window
//! instead of accepting. `trade_swap` is a thin caller over this.

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
    /// The confirm screen differs from the agreed lots.
    ConfirmMismatch,
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
        let carried: u32 = self.inventory().get(&obj_id).copied().unwrap_or(0);
        if carried == count && count > 0 {
            let (com, _) = self.backpack();
            let Some(com) = com else { return Ok(false) };
            let Some(slot) = self.backpack().1.iter().find(|(_, (held, _))| *held == obj_id).map(|(slot, _)| *slot) else {
                return Ok(false);
            };
            self.inv_button(4, obj_id, slot, com)?;
            let target = self.world.ticks + timeout.min(120);
            while self.world.ticks < target {
                self.pump(5)?;
                if self.offered_of(obj_id) >= count {
                    return Ok(true);
                }
            }
            return Ok(self.offered_of(obj_id) >= count);
        }
        self.trade_offer(obj_id, count, timeout)
    }

    fn offered_of(&self, obj_id: u16) -> u32 {
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
        let full_give = self.with_coins(give, coins_give);
        let full_want = self.with_coins(want, coins_want);
        for (obj, count) in &full_give {
            if self.inventory().get(obj).copied().unwrap_or(0) < *count {
                return Ok(ExchangeOutcome::Short);
            }
        }
        let deadline = self.world.ticks + timeout;
        let seen = self.world.messages.len();
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
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::PartnerMismatch);
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
        let confirm_coms = [
            (self.tables.com_id)("tradeconfirm:inv1").unwrap_or(3542),
            (self.tables.com_id)("tradeconfirm:inv2").unwrap_or(3538),
        ];
        let other_coms = [
            (self.tables.com_id)("tradeconfirm:otherinv1").unwrap_or(3532),
            (self.tables.com_id)("tradeconfirm:otherinv2").unwrap_or(3539),
        ];
        let aggregate = |coms: &[u16; 2]| {
            let mut acc = HashMap::new();
            for com in coms {
                if let Some(slots) = self.world.inv.get(com) {
                    for (held, n) in slots.values() {
                        *acc.entry(*held).or_insert(0) += *n;
                    }
                }
            }
            acc
        };
        if !Self::lots_match(&aggregate(&confirm_coms), &full_give)
            || !Self::lots_match(&aggregate(&other_coms), &full_want)
        {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::ConfirmMismatch);
        }
        while self.world.ticks < deadline && self.trade_modal().is_some() {
            let left = deadline.saturating_sub(self.world.ticks).max(1);
            self.trade_accept(left.min(300))?;
            self.pump(5)?;
        }
        if self.trade_modal().is_some() {
            let _ = self.trade_close();
            return Ok(ExchangeOutcome::ConfirmMismatch);
        }
        Ok(ExchangeOutcome::Settled)
    }
}

#[cfg(test)]
mod tests {
    use crate::client::Bot;
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
}
