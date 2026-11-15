//! Market locks (user rule, 2026-11-13): what a bot's open orders and
//! in-flight deals promise stays in its bank. "Verrouiller soit l'argent
//! soit les objets pour plus qu'il y touche en dehors d'une procedure de
//! trade." The daemon computes them from the order book and pushes them
//! to the worker; every bank withdrawal is capped so the locked part never
//! leaves the bank, except inside the trade procedure (`trading`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::Bot;

/// Coins object id (the locked purse rides apart from the items).
pub const COINS: u16 = 995;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Locks {
    /// Base object id -> units promised to open sells and seller deals.
    pub items: HashMap<u16, u32>,
    /// Coins promised to open bids and buyer deals.
    pub coins: u32,
    /// Set by the trade procedure while it withdraws its own lots.
    pub trading: bool,
    /// Objects the running script keeps in its pack (a quest's talisman,
    /// package, notes and brought items): no deposit-all banks them and
    /// no market handover trades them (live 2026-11-14, bh99tw2f: a
    /// delivery mid-quest banked the Duke's air talisman). Set by the
    /// script, never by the daemon's pushes.
    pub held: Vec<u16>,
}

/// Shared between the bot and its session, which receives the daemon's
/// pushes while a script holds the bot.
pub type SharedLocks = Arc<Mutex<Locks>>;

/// What a deposit-all leaves in the pack: the caller's keep list plus
/// the script's held objects. Pure, locked by test.
pub fn deposit_keep(keep: &[u16], held: &[u16]) -> Vec<u16> {
    let mut out = keep.to_vec();
    out.extend(held.iter().copied().filter(|object| !keep.contains(object)));
    out
}

/// Units of an object a withdrawal may take: what the bank holds beyond
/// the locked part. Pure, so the rule is locked by test.
pub fn withdraw_cap(banked: u32, locked: u32, trading: bool) -> u32 {
    if trading {
        banked
    } else {
        banked.saturating_sub(locked)
    }
}

/// Lowers the lock of `obj` by `units` (never under 0; the entry goes
/// at 0). Pure, locked by test.
pub fn release(locks: &mut Locks, obj: u16, units: u32) {
    if obj == COINS {
        locks.coins = locks.coins.saturating_sub(units);
        return;
    }
    let left = locks.items.get(&obj).copied().unwrap_or(0).saturating_sub(units);
    if left == 0 {
        locks.items.remove(&obj);
    } else {
        locks.items.insert(obj, left);
    }
}

impl Bot {
    /// Locked units of `obj` (coins included), 0 inside a trade.
    pub fn locked(&self, obj: u16) -> u32 {
        let locks = self.locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if locks.trading {
            return 0;
        }
        if obj == COINS {
            locks.coins
        } else {
            locks.items.get(&obj).copied().unwrap_or(0)
        }
    }

    /// Units of `obj` a withdrawal may take from a bank holding `banked`.
    pub fn withdrawable(&self, obj: u16, banked: u32) -> u32 {
        let trading = self.locks.lock().map(|locks| locks.trading).unwrap_or(false);
        withdraw_cap(banked, self.locked(obj), trading)
    }

    /// Frees `units` of the lock on base object `obj` after this worker
    /// cancelled the order that promised them (live 2026-11-15: cleanup
    /// cancelled its listings yet every withdraw still read the pushed
    /// lock, 56 bank types stayed). The daemon's next push recomputes.
    pub fn release_locked(&self, obj: u16, units: u32) {
        if let Ok(mut locks) = self.locks.lock() {
            release(&mut locks, obj, units);
        }
    }

    /// Opens or closes the trade window of the locks: only the market
    /// handover takes its own promised lots out of the bank.
    pub fn set_trading(&self, on: bool) {
        if let Ok(mut locks) = self.locks.lock() {
            locks.trading = on;
        }
    }

    /// Keeps `objects` in the pack for the running script (empty: none).
    pub fn hold_in_pack(&self, objects: &[u16]) {
        if let Ok(mut locks) = self.locks.lock() {
            locks.held = objects.to_vec();
        }
    }

    /// Objects the running script keeps in its pack.
    pub fn pack_holds(&self) -> Vec<u16> {
        self.locks.lock().map(|locks| locks.held.clone()).unwrap_or_default()
    }

    /// Replaces the pushed locks (daemon side), keeping the trade flag.
    pub fn replace_locks(shared: &SharedLocks, items: HashMap<u16, u32>, coins: u32) {
        if let Ok(mut locks) = shared.lock() {
            locks.items = items;
            locks.coins = coins;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{deposit_keep, release, withdraw_cap, Bot, Locks, COINS};
    use std::collections::HashMap;

    #[test]
    fn a_cancelled_order_releases_its_lock() {
        let mut locks = Locks::default();
        locks.items.insert(526, 4);
        release(&mut locks, 526, 3);
        assert_eq!(locks.items.get(&526), Some(&1));
        release(&mut locks, 526, 9);
        assert!(!locks.items.contains_key(&526), "never under 0, entry gone");
    }

    #[test]
    fn locked_part_never_leaves_outside_a_trade() {
        assert_eq!(withdraw_cap(130, 23, false), 107, "130 shrimps, 23 sold: 107 usable");
        assert_eq!(withdraw_cap(10, 23, false), 0, "fewer than locked: nothing");
        assert_eq!(withdraw_cap(130, 23, true), 130, "the trade takes its lots");
    }

    #[test]
    fn bot_reads_items_and_coins_apart() {
        let bot = Bot::new("127.0.0.1", 43594, ".", ".");
        Bot::replace_locks(&bot.locks, HashMap::from([(317u16, 23u32)]), 40);
        assert_eq!(bot.locked(317), 23);
        assert_eq!(bot.locked(COINS), 40);
        assert_eq!(bot.withdrawable(317, 130), 107);
        bot.set_trading(true);
        assert_eq!(bot.locked(317), 0, "inside a trade nothing is locked");
        assert_eq!(bot.withdrawable(COINS, 50), 50);
        bot.set_trading(false);
        assert_eq!(bot.withdrawable(COINS, 50), 10);
    }

    /// A held quest object survives every deposit-all; the daemon's lock
    /// pushes never clear it.
    #[test]
    fn held_objects_stay_in_the_pack() {
        let bot = Bot::new("127.0.0.1", 43594, ".", ".");
        bot.hold_in_pack(&[1438, 290]);
        Bot::replace_locks(&bot.locks, HashMap::from([(1438u16, 9u32)]), 0);
        assert_eq!(bot.pack_holds(), vec![1438, 290], "pushes keep the holds");
        assert_eq!(deposit_keep(&[1265], &bot.pack_holds()), vec![1265, 1438, 290]);
        assert_eq!(deposit_keep(&[1438], &[1438]), vec![1438], "no duplicate");
        bot.hold_in_pack(&[]);
        assert!(bot.pack_holds().is_empty());
    }
}
