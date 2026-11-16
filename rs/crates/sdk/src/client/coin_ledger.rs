//! Coin ledger (audit 2026-11-15, chantier 10: "mesure économique
//! fiable, provenance par événement"). Every change of the coins carried
//! in the pack is one flow, attributed to the action that caused it:
//! the action layer names its context (`Bot::coin_context`: a shop
//! trade, a player trade, a fee, a loot pickup, an alchemy cast, a
//! dialogue purchase) for a few ticks; a change with the bank open is a
//! transfer (pack ↔ bank), never a flow. What no context explains is
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
    pub flows: Vec<CoinFlow>,
}

impl CoinLedger {
    /// The action context in force (kind, until tick), for the item
    /// ledger beside it.
    pub fn context(&self) -> Option<(&'static str, u64)> {
        self.context
    }

    /// Names the context of the coin changes until `until` (a tick).
    pub fn set_context(&mut self, kind: &'static str, until: u64) {
        self.context = Some((kind, until));
    }

    /// One observation of the carried coins at `tick` (`bank_open`: the
    /// change is a transfer). Records a flow when the count moved.
    pub fn observe(&mut self, pack: u32, bank_open: bool, tick: u64) {
        let Some(last) = self.last_pack.replace(pack) else { return };
        if last == pack || bank_open {
            return;
        }
        let kind = match self.context {
            Some((kind, until)) if tick <= until => kind,
            _ => kind::UNATTRIBUTED,
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
        ledger.set_context(kind::SHOP, 20);
        ledger.observe(160, false, 10);
        ledger.observe(130, false, 12);
        ledger.observe(125, false, 50);
        assert_eq!(ledger.totals_since(0), vec![(kind::SHOP, 30), (kind::UNATTRIBUTED, -5)]);
    }

    #[test]
    fn the_first_sight_is_a_baseline() {
        let mut ledger = CoinLedger::default();
        ledger.observe(5_000, false, 1);
        assert!(ledger.flows.is_empty());
        assert!(ledger.totals_since(0).is_empty());
    }
}
