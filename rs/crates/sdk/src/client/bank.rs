//! Bank transfers: the one primitive behind every deposit and withdrawal.
//!
//! Server truth (`content/scripts/interface_bank/scripts/bank.rs2`,
//! `bank_deposit.rs2`): both bank containers answer five ops, like the
//! player's right-click menu (`bank_main.if`/`bank_side.if` option1..5):
//! 1 = one, 2 = five, 3 = ten, 4 = all (`^max_32bit_int`), 5 = X
//! (`p_countdialog`, then `last_int`). Every op moves
//! `min(requested, inv_total(container, obj))` across ALL slots of the
//! object at once (`inv_moveitem_uncert`), so Deposit-All empties every
//! slot of a non-stackable object in one press and Withdraw-X lands up to
//! the pack room in one press. Client-TS (274) sends INV_BUTTON1..5
//! (obj, slot, com), and answers P_COUNTDIALOG with RESUME_P_COUNTDIALOG
//! (p4). One press per object and per transfer, like a player: the old
//! paths pressed Deposit-All once per slot (27 presses for a pack of two
//! fish types) and withdrew with 10/5/1 round trips (60 coins = 6 presses).

use std::collections::HashMap;

use crate::client::actions::COUNT_DIALOG_TICKS;
use crate::client::Bot;
use crate::error::{Error, Result};

/// One bank button, as a player picks it from the item's menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankButton {
    One,
    Five,
    Ten,
    All,
    /// "X": the count dialog answered with this amount.
    Amount(u32),
}

impl BankButton {
    /// INV_BUTTON op of the button (bank_main.if / bank_side.if option order).
    pub fn op(self) -> u8 {
        match self {
            BankButton::One => 1,
            BankButton::Five => 2,
            BankButton::Ten => 3,
            BankButton::All => 4,
            BankButton::Amount(_) => 5,
        }
    }
}

/// The single button that moves exactly `want` units out of a stack of
/// `stack` (the container total of that object): All when the whole
/// stack goes, 1/5/10 when the amount is exactly one of them, else X with
/// the amount. `None` when nothing moves. Pure: locked by tests.
pub fn bank_button(want: u32, stack: u32) -> Option<BankButton> {
    if want == 0 || stack == 0 {
        return None;
    }
    if want >= stack {
        return Some(BankButton::All);
    }
    Some(match want {
        1 => BankButton::One,
        5 => BankButton::Five,
        10 => BankButton::Ten,
        other => BankButton::Amount(other),
    })
}

/// Server lines that refuse a bank transfer for good (pack or bank full,
/// unbankable, F2P type cap, full stack, note refusal): stop waiting.
const TRANSFER_REFUSALS: [&str; 7] = [
    "enough inventory space",
    "not going to be able to carry",
    "subscribe for a members account",
    "magical force prevents you from banking",
    "full stack of that item",
    "enough space in your bank",
    "can not be withdrawn as a note",
];

/// Ticks a refusal waits for the part of a transfer that still fits.
const REFUSAL_SETTLE_TICKS: u64 = 3;

/// True when a refusal line arrived since `seen`. Pure on the history.
pub fn transfer_refused(messages: &[String], seen: usize) -> bool {
    messages.get(seen..).unwrap_or(&[]).iter().any(|line| {
        let lowered = line.to_lowercase();
        TRANSFER_REFUSALS.iter().any(|refusal| lowered.contains(refusal))
    })
}

impl Bot {
    pub fn bank_items(&self) -> HashMap<u16, u32> {
        crate::snapshot::bank_items(&self.world.inv, (self.tables.com_id)("bank_main:inv"))
    }

    fn bank_com(&self, name: &str) -> Result<u16> {
        (self.tables.com_id)(name).ok_or_else(|| Error::State(format!("{name} missing from component table")))
    }

    /// First slot of `obj` in the open bank stack and its total there.
    fn bank_stack(&self, main_com: u16, obj: u16) -> (Option<u8>, u32) {
        let Some(slots) = self.world.inv.get(&main_com) else { return (None, 0) };
        let mut held: Vec<(u8, u32)> = slots.iter().filter(|(_, (id, _))| *id == obj).map(|(slot, (_, count))| (*slot, *count)).collect();
        held.sort_unstable();
        let total = held.iter().map(|(_, count)| *count).fold(0u32, u32::saturating_add);
        (held.first().map(|(slot, _)| *slot), total)
    }

    /// Press one bank button on `obj` at `slot` of `com`. X waits for the
    /// server's count dialog and answers it; a dialog that never comes
    /// (bank closed, click lost) fails typed instead of a blind resume.
    fn press_bank_button(&mut self, button: BankButton, obj: u16, slot: u8, com: u16) -> Result<()> {
        let BankButton::Amount(amount) = button else {
            return self.inv_button(button.op(), obj, slot, com);
        };
        self.world.count_dialog = false;
        self.inv_button(button.op(), obj, slot, com)?;
        let asked = self.world.ticks + COUNT_DIALOG_TICKS;
        while self.world.ticks < asked && !self.world.count_dialog {
            self.pump(1)?;
        }
        if !self.world.count_dialog {
            return Err(Error::State(format!("bank X of {amount} asked no count in {COUNT_DIALOG_TICKS} ticks at {:?}", self.position())));
        }
        self.world.count_dialog = false;
        self.resume_pcount(i32::try_from(amount).unwrap_or(i32::MAX))
    }

    /// Pump until the pack count of `obj` leaves `before` (a transfer
    /// landed), a refusal arrives, or `timeout` ticks pass. Units moved.
    fn await_pack_change(&mut self, obj: u16, before: u32, seen: usize, timeout: u64) -> Result<(u32, bool)> {
        let deadline = self.world.ticks + timeout;
        while self.world.ticks < deadline {
            self.pump(1)?;
            let now = self.pack_count(obj);
            if now != before {
                return Ok((now.abs_diff(before), transfer_refused(&self.world.messages, seen)));
            }
            if transfer_refused(&self.world.messages, seen) {
                // A partial transfer ("not enough inventory space to
                // withdraw that many" with the part that fits) can stream
                // its message before the pack update (live 2026-11-16:
                // 15 of 58 shrimps landed after the line): settle briefly.
                let settle = self.world.ticks + REFUSAL_SETTLE_TICKS;
                while self.world.ticks < settle && self.pack_count(obj) == before {
                    self.pump(1)?;
                }
                return Ok((self.pack_count(obj).abs_diff(before), true));
            }
        }
        Ok((0, false))
    }

    /// Deposit `count` of `obj` from the pack (`u32::MAX` = every unit)
    /// in ONE press: All when the whole carried amount goes, 1/5/10 when
    /// exact, else X. Units that left the pack; 0 on a refusal (bank
    /// full, unbankable) or when none is carried.
    pub fn bank_deposit(&mut self, obj: u16, count: u32, timeout: u64) -> Result<u32> {
        let side_com = self.bank_com("bank_side:inv")?;
        let carried = self.pack_count(obj);
        let Some(button) = bank_button(count, carried) else { return Ok(0) };
        self.act(format!("bank_deposit {} x{} ({button:?})", (self.tables.obj_name)(obj).unwrap_or_default(), count.min(carried)));
        let Some(slot) = self.backpack().1.iter().filter(|(_, (held, _))| *held == obj).map(|(slot, _)| *slot).min() else {
            return Ok(0);
        };
        let seen = self.world.messages.len();
        self.press_bank_button(button, obj, slot, side_com)?;
        let (moved, refused) = self.await_pack_change(obj, carried, seen, timeout)?;
        if moved == 0 && !refused {
            return Err(Error::Timeout(format!("deposit of {obj} timed out at {:?} ({timeout} ticks)", self.position())));
        }
        Ok(moved)
    }

    /// Deposit the backpack: one Deposit-All per object type (the server
    /// empties every slot of it at once), one burst, then one retry of
    /// the leftovers. Tick-bounded. The running script's held objects
    /// (`hold_in_pack`) always stay.
    pub fn bank_deposit_all(&mut self, side_com: u16, keep_ids: &[u16], timeout: u64) -> Result<bool> {
        let kept = super::locks::deposit_keep(keep_ids, &self.pack_holds());
        let keep_ids: &[u16] = &kept;
        self.act(format!("bank_deposit_all keep={} timeout={timeout}", keep_ids.len()));
        for _ in 0..2 {
            let pending = deposit_presses(&self.backpack().1, keep_ids);
            if pending.is_empty() {
                return Ok(true);
            }
            for (slot, held) in pending {
                self.inv_button(BankButton::All.op(), held, slot, side_com)?;
            }
            let deadline = self.world.ticks + timeout;
            while self.world.ticks < deadline {
                self.pump(2)?;
                let (_, now) = self.backpack();
                if now.iter().all(|(_, (held, _))| keep_ids.contains(held)) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Withdraw `count` of `obj` (exact: All only when the whole usable
    /// stack goes, 1/5/10 when exact, else X), honouring market locks.
    pub fn bank_withdraw(&mut self, obj_id: u16, count: u32, timeout: u64) -> Result<u32> {
        self.act(format!("bank_withdraw {} x{count}", (self.tables.obj_name)(obj_id).unwrap_or_default()));
        self.withdraw_as(obj_id, obj_id, count, timeout)
    }

    /// Withdraw from the bank stack of `bank_obj` and watch `pack_obj`
    /// land in the pack: the same object, or its banknote when the note
    /// mode is on (the bank only ever holds the base object). One press
    /// per transfer; a full pack ends the withdrawal quietly with what
    /// fits. Returns the pack count of `pack_obj`.
    pub(crate) fn withdraw_as(&mut self, bank_obj: u16, pack_obj: u16, count: u32, timeout: u64) -> Result<u32> {
        let main_com = self.bank_com("bank_main:inv")?;
        // Market locks: the part promised to open orders stays banked
        // outside the trade procedure (user rule 2026-11-13). The cap
        // keeps `remaining` below the stack while a lock holds, so All
        // (which would take the promised part too) is never chosen then.
        let (_, banked) = self.bank_stack(main_com, bank_obj);
        let locked = self.locked(bank_obj);
        let mut remaining = count.min(self.withdrawable(bank_obj, banked));
        if remaining == 0 {
            if locked > 0 {
                self.log(&format!("bank_withdraw {bank_obj}: {locked} locked by open market orders, nothing usable"));
            }
            return Ok(self.pack_count(pack_obj));
        }
        while remaining > 0 {
            let (slot, stack) = self.bank_stack(main_com, bank_obj);
            let (Some(slot), Some(button)) = (slot, bank_button(remaining, stack)) else { break };
            let before = self.pack_count(pack_obj);
            let seen = self.world.messages.len();
            self.press_bank_button(button, bank_obj, slot, main_com)?;
            let (moved, refused) = self.await_pack_change(pack_obj, before, seen, timeout)?;
            if moved == 0 && !refused {
                if self.backpack().1.len() >= 28 {
                    self.log(&format!("bank_withdraw {bank_obj}: pack full, {remaining} left in the bank"));
                    break;
                }
                return Err(Error::Timeout(format!("withdraw of {bank_obj} timed out at {:?} ({timeout} ticks)", self.position())));
            }
            remaining = remaining.saturating_sub(moved);
            if refused {
                if remaining > 0 {
                    self.log(&format!("bank_withdraw {bank_obj}: refused by the server, {remaining} left in the bank"));
                }
                break;
            }
        }
        Ok(self.pack_count(pack_obj))
    }
}

/// One Deposit-All press per object type outside `keep`: its lowest
/// slot. Pure so the press count is locked by tests.
pub fn deposit_presses(slots: &[(u8, (u16, u32))], keep: &[u16]) -> Vec<(u8, u16)> {
    let mut first: HashMap<u16, u8> = HashMap::new();
    for (slot, (held, _)) in slots {
        if keep.contains(held) {
            continue;
        }
        let entry = first.entry(*held).or_insert(*slot);
        *entry = (*entry).min(*slot);
    }
    let mut presses: Vec<(u8, u16)> = first.into_iter().map(|(held, slot)| (slot, held)).collect();
    presses.sort_unstable();
    presses
}

#[cfg(test)]
#[path = "bank_tests.rs"]
mod tests;
