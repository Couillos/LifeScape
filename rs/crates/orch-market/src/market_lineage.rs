#![forbid(unsafe_code)]
//! Order lineage and motive (plan marché M1, §3.1/§3.5/§3.6).
//!
//! Every order carries a MOTIVE (why it exists) and belongs to a LINEAGE
//! (its first order and t0, kept by every amend: `lifescape_store`
//! schema 6). The motive fixes the horizon of the order in ticks (§3.5:
//! the time its concession takes to reach its limit, lot M3), the
//! lineage its real age (`market_demand`), its rank in the coverage
//! audit and its life in `orchestrator economy`.
//!
//! Credibility (§3.6, C5): no bid under the public floor of its object
//! (low alchemy value; a quarter of the current traded price): a bot
//! that cannot offer that saves first instead (a rune axe bid at 1 coin
//! is noise in the demand signals and CPU in the walk).

/// Blocking need of a graph node: its market leg, its standing bid
/// beside production (D-B), a quest's object.
pub const NEED: &str = "need";
/// The gear plan's pieces (`brain_gear`).
pub const GEAR: &str = "gear";
/// Tool wishes (`brain_wishlist`).
pub const TOOL: &str = "tool";
/// Stock bought ahead (alchemy stock, future inputs).
pub const STOCK: &str = "stock";
/// A producer's surplus listed for sale.
pub const SURPLUS: &str = "surplus";
/// Loot or found objects listed for sale.
pub const LOOT: &str = "loot";
/// A dedicated market maker's quote (plan marché Q6, `market_maker`):
/// fixed price, requoted by its own pass, never on the concession curve.
pub const MARKET_MAKE: &str = "market_make";
/// The product of a 100 % market lot listed for sale (plan rendements
/// R5, `trade_craft`): inputs bought, crafted, sold; the horizon of a
/// producer's surplus (its reserve is the lot's own cost).
pub const TRADE: &str = "trade";
/// A merchant arbitrage purchase (plan équilibre D2, `brain_arbitrage`):
/// units bought under what an NPC shop pays, net of the trip, for that
/// shop; fixed at the dearest ask its lot was priced on, resized by its
/// own pass.
pub const ARBITRAGE: &str = "arbitrage";

/// Every motive an order may carry (empty = not said, a worker's order).
pub const MOTIVES: [&str; 9] = [NEED, GEAR, TOOL, STOCK, SURPLUS, LOOT, MARKET_MAKE, TRADE, ARBITRAGE];

/// Rejects an unknown motive (typed refusal, never a silent default).
pub fn check_motive(motive: &str) -> Result<(), String> {
    if motive.is_empty() || MOTIVES.contains(&motive) {
        Ok(())
    } else {
        Err(format!("unknown order motive '{motive}' (known: {})", MOTIVES.join(", ")))
    }
}

/// Horizon of a motive in ticks (§3.5 defaults, calibrated in ticks: 60
/// ms today, ×10 at 600 ms). An order reaches its limit at its horizon
/// and stays there while its motive exists (Q4: a buyer stays, a seller
/// withdraws, wave 2).
pub fn horizon_ticks(motive: &str, sell: bool) -> u64 {
    match motive {
        NEED | ARBITRAGE => 6_000,
        GEAR | TOOL | SURPLUS | MARKET_MAKE | TRADE => 36_000,
        STOCK | LOOT => 72_000,
        _ if sell => 36_000,
        _ => 6_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizons_follow_the_motive() {
        assert_eq!(horizon_ticks(NEED, false), 6_000);
        assert_eq!(horizon_ticks(GEAR, false), 36_000);
        assert_eq!(horizon_ticks(STOCK, false), 72_000);
        assert_eq!(horizon_ticks(LOOT, true), 72_000);
        assert_eq!(horizon_ticks(TRADE, true), horizon_ticks(SURPLUS, true), "a trade lot sells like a producer's surplus");
        assert_eq!(horizon_ticks("", true), 36_000, "an unsaid sale: surplus");
        assert_eq!(horizon_ticks("", false), 6_000, "an unsaid bid: a need");
    }

    #[test]
    fn unknown_motives_are_refused() {
        assert!(check_motive("").is_ok());
        assert!(check_motive(GEAR).is_ok());
        assert!(check_motive(TRADE).is_ok());
        assert!(check_motive(ARBITRAGE).is_ok());
        assert!(check_motive("whim").is_err());
    }
}
