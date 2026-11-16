//! Coin ledger (audit 2026-11-15, chantier 10: "mesure économique
//! fiable, provenance par événement"). Every change of the coins carried
//! in the pack is one flow, attributed to the action that caused it:
//! the action layer names its context (`Bot::coin_context`: a shop
//! trade, a player trade, a fee, a loot pickup, an alchemy cast, a
//! dialogue purchase) for a few ticks; a change with the bank open, or
//! closed less than `item_ledger::BANK_GRACE_TICKS` ago with no context
//! set since, is a transfer (pack ↔ bank), never a flow. What no context explains is
//! `unattributed`, kept visible. The session report reads the flows of a
//! run instead of guessing from net holdings.

#![forbid(unsafe_code)]

/// Ticks a named context covers after its action.
pub const CONTEXT_TICKS: u64 = 40;

/// One attributed change of the carried coins.
#[derive(Debug, Clone, PartialEq)]
pub struct CoinFlow {
    pub kind: &'static str,
    pub delta: i64,
    pub tick: u64,
}

/// Flow kinds the action layer names.
pub mod kind {
    /// NPC shop trade (buy: negative, sell: positive).
    pub const SHOP: &str = "shop";
    /// Player trade (the market handover).
    pub const TRADE: &str = "trade";
    /// Toll, fare, tanning fee, ferry: coins paid for a service.
    pub const FEE: &str = "fee";
    /// Coins picked off the ground (a drop or a spawn).
    pub const LOOT: &str = "loot";
    /// Alchemy: the spell's coins.
    pub const ALCH: &str = "alch";
    /// An NPC selling through its dialogue (beer, kebab, rope).
    pub const DIALOGUE: &str = "dialogue";
    /// No context explains it.
    pub const UNATTRIBUTED: &str = "unattributed";
}

#[derive(Debug, Clone, Default)]
pub struct CoinLedger {
    last_pack: Option<u32>,
    context: Option<(&'static str, u64)>,
    /// Tick the context in force was set at.
    context_from: u64,
    /// Last tick the bank was seen open.
    bank_seen: Option<u64>,
    pub flows: Vec<CoinFlow>,
}

impl CoinLedger {
    /// The action context in force (kind, until tick), for the item
    /// ledger beside it.
    pub fn context(&self) -> Option<(&'static str, u64)> {
        self.context
    }

    /// Names the context of the coin changes, set at tick `from`, until
    /// `until` (a tick).
    pub fn set_context(&mut self, kind: &'static str, from: u64, until: u64) {
        self.context = Some((kind, until));
        self.context_from = from;
    }

    /// One observation of the carried coins at `tick` (`bank_open`: the
    /// change is a transfer; closed less than `BANK_GRACE_TICKS` ago:
    /// still one, unless a context set since names it, as the item
    /// ledger). Records a flow when the count moved.
    pub fn observe(&mut self, pack: u32, bank_open: bool, tick: u64) {
        if bank_open {
            self.bank_seen = Some(tick);
        }
        let Some(last) = self.last_pack.replace(pack) else { return };
        if last == pack || bank_open {
            return;
        }
        let context = self.context.filter(|(_, until)| tick <= *until);
        let grace = self.bank_seen.filter(|at| tick <= at + super::item_ledger::BANK_GRACE_TICKS);
        if grace.is_some_and(|open_at| context.is_none() || self.context_from < open_at) {
            return;
        }
        let kind = match context {
            Some((kind, _)) => kind,
            None => kind::UNATTRIBUTED,
        };
        self.flows.push(CoinFlow { kind, delta: i64::from(pack) - i64::from(last), tick });
    }

    /// Net coins per kind over the flows from index `start` on.
    pub fn totals_since(&self, start: usize) -> Vec<(&'static str, i64)> {
        let mut out: Vec<(&'static str, i64)> = Vec::new();
        for flow in self.flows.iter().skip(start) {
            match out.iter_mut().find(|(kind, _)| *kind == flow.kind) {
                Some(entry) => entry.1 += flow.delta,
                None => out.push((flow.kind, flow.delta)),
            }
        }
        out.retain(|(_, net)| *net != 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{kind, CoinLedger};

    #[test]
    fn a_shop_sale_is_attributed_and_the_bank_is_a_transfer() {
        let mut ledger = CoinLedger::default();
        ledger.observe(100, false, 1);
        ledger.observe(0, true, 2);
        ledger.observe(100, true, 3);
        assert!(ledger.flows.is_empty(), "deposit and withdraw are transfers");
        ledger.set_context(kind::SHOP, 9, 20);
        ledger.observe(160, false, 10);
        ledger.observe(130, false, 12);
        ledger.observe(125, false, 50);
        assert_eq!(ledger.totals_since(0), vec![(kind::SHOP, 30), (kind::UNATTRIBUTED, -5)]);
    }

    /// Live 2026-11-16 (alice_knight shop:buy t=1306-1308): withdraw 6
    /// coins, close at once. The ledgers observe on tick boundaries, and
    /// the close's stop-transmit lands before the next player info: the
    /// withdrawal is first seen with the bank already closed. Inside the
    /// close grace it stays a transfer, unless a context set since the
    /// bank was last seen open names it.
    #[test]
    fn a_withdrawal_seen_after_the_close_is_a_transfer() {
        let mut ledger = CoinLedger::default();
        ledger.observe(0, false, 1);
        ledger.set_context(kind::SHOP, 2, 40);
        ledger.observe(0, true, 5);
        ledger.observe(6, false, 6);
        assert!(ledger.flows.is_empty(), "late withdrawal is a transfer: {:?}", ledger.flows);
        ledger.set_context(kind::FEE, 6, 6 + super::CONTEXT_TICKS);
        ledger.observe(0, false, 7);
        assert_eq!(ledger.totals_since(0), vec![(kind::FEE, -6)], "a fee paid right after the close is a fee");
    }

    #[test]
    fn the_first_sight_is_a_baseline() {
        let mut ledger = CoinLedger::default();
        ledger.observe(5_000, false, 1);
        assert!(ledger.flows.is_empty());
        assert!(ledger.totals_since(0).is_empty());
    }
}
