//! Keepers sold out (user 2026-11-20, « une rune que personne ne vend
//! est indisponible, jamais gratuite »): the server data gives each F2P
//! shop's BASE stock and restock rate (`gamedata::shop_buy`), never what
//! the fleet left on the shelf. The shared shops (`scope=shared`, one
//! stock for every player) are emptied by the fleet: live 2026-11-20 the
//! chaos runes of Aubury and Betty (1 000 each, one back every 100
//! ticks) were gone, the market traded them at ~2 000 coins (keepers:
//! 15 to 94), and every guide still priced them at the keepers' 15.
//!
//! The market tells: a bot buys at the keeper first whenever the asks
//! cost more (`resolve_obtain`, rule 0), so a TRADED price above every
//! keeper's dearest unit (stock 1) says the buyers found the shelves
//! empty. Such keepers count no units in the GUIDES (their purchase walk
//! and availability, `guide_engine::Engine::shop_steps`): the market
//! alone sells there, at its asks and its current price. A never-traded
//! object, or one traded at or under a keeper's price, keeps the
//! keepers' window, so does a window without end (no stock bound, the
//! fabricated worlds' flat shops). An obtain still tries the keeper: its script reads
//! the real shelf, where a dear market proves nothing (live: 179 steel
//! arrows bought at Lowe's while the market traded them at ~300).

/// True when the keepers of an object are sold out: it traded
/// (`traded`) at `market_price` per unit, above every keeper's price
/// (`shop_prices`, the window's unit prices). False without a keeper.
pub fn keepers_sold_out(market_price: f64, traded: bool, shop_prices: &[f64]) -> bool {
    let dearest = shop_prices.iter().copied().filter(|price| price.is_finite() && *price >= 0.0).fold(None, |most: Option<f64>, price| Some(most.map_or(price, |known| known.max(price))));
    traded && dearest.is_some_and(|dearest| market_price > dearest)
}

#[cfg(test)]
mod tests {
    use super::keepers_sold_out;

    /// Live 2026-11-20: chaos runes traded at ~2 000, keepers 15 to 94:
    /// sold out. Traded at their price, or never traded: stocked.
    #[test]
    fn a_market_above_every_keeper_proves_them_sold_out() {
        assert!(keepers_sold_out(2000.0, true, &[15.0, 94.0]));
        assert!(!keepers_sold_out(20.0, true, &[15.0, 94.0]), "under the dearest unit: the keepers may sell");
        assert!(!keepers_sold_out(94.0, true, &[15.0, 94.0]), "at the dearest unit: still the keeper's price");
        assert!(!keepers_sold_out(2000.0, false, &[15.0, 94.0]), "never traded: no proof");
        assert!(!keepers_sold_out(2000.0, true, &[]), "no keeper: nothing to empty");
    }
}
