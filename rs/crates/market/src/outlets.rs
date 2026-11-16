#![forbid(unsafe_code)]
//! Outside options of a seller (plan marché Q3, user decision
//! 2026-11-16): an object the market pays LESS for than an NPC shop is
//! sold to the shop, like a player emptying his loot at the general
//! store (an exception to D-A, `docs/decisions.md`). What the market
//! pays is public: the best resting bid, the current traded price. A
//! never-traded object with no bid has no market price: the shop's
//! offer wins. Never a sale under what the market pays.

use crate::price::Reference;

/// What the market pays for one unit now: the best bid, or the traded
/// current price when richer; 0 when it pays nothing known.
pub fn market_pays(best_bid: Option<i64>, reference: Reference) -> i64 {
    let traded = if reference.traded { reference.price } else { 0 };
    best_bid.unwrap_or(0).max(traded).max(0)
}

/// True when an NPC shop paying `shop_value` per unit beats the market
/// (strictly: an equal price stays on the market, no coins created).
pub fn npc_sale_beats_market(shop_value: i64, best_bid: Option<i64>, reference: Reference) -> bool {
    shop_value >= 1 && shop_value > market_pays(best_bid, reference)
}

/// Least coins one unit may fetch at the shop: one more than the market
/// pays (never a sale at a loss against the market), at least 1.
pub fn npc_sale_min_unit(best_bid: Option<i64>, reference: Reference) -> i64 {
    market_pays(best_bid, reference) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNTRADED: Reference = Reference { price: 18, traded: false };

    /// Q3 test: the shop only when the market pays less than it.
    #[test]
    fn npc_sales_only_when_the_market_pays_less() {
        let traded = |price| Reference { price, traded: true };
        assert!(npc_sale_beats_market(14, None, UNTRADED), "iron daggers nobody ever bought: the general store");
        assert!(!npc_sale_beats_market(14, Some(14), UNTRADED), "an equal bid keeps it on the market");
        assert!(!npc_sale_beats_market(14, Some(20), UNTRADED), "a richer bid wins");
        assert!(!npc_sale_beats_market(14, None, traded(30)), "traded at 30: the market");
        assert!(npc_sale_beats_market(14, Some(3), traded(5)), "the market pays 5: the shop's 14");
        assert!(!npc_sale_beats_market(0, None, UNTRADED), "a shop paying nothing never wins");
        assert_eq!(npc_sale_min_unit(Some(3), traded(5)), 6, "never a unit at or under the market");
        assert_eq!(npc_sale_min_unit(None, UNTRADED), 1);
    }
}
