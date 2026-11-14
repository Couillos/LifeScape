#![forbid(unsafe_code)]
//! Sale-price estimate for the rates (PRD3 lot 2), split from
//! `rates_service` (rule 4): trades first, then the waiting buyers, then
//! the 1-coin default. NPC shop prices never value anything.

use lifescape_store::Store;

/// Estimated sale price of an item display name: its current price
/// (user 2026-11-14, `lifescape_market::current_price`: median of the
/// last five fills, any age, clamped by the impact prices of the
/// resting book; never traded: the book, else 1 coin). Three indexed
/// reads (last five fills, the best bids then the best asks until the
/// impact size is covered), never the item's whole history. The flag marks a
/// price no trade backs yet (`price_prior`). NPC shop prices never value
/// anything: nobody sells to merchants.
pub fn price_estimate(store: &Store, item: &str) -> (f64, bool) {
    // One name lookup, never the whole catalogue per call (CPU
    // 2026-11-14: `profit_rows` asked ~900 times per call).
    let found_id = match store.item_id_by_name(item) {
        Ok(found) => found,
        Err(_) => return (1.0, true),
    };
    if item.eq_ignore_ascii_case("coins") {
        return (1.0, false);
    }
    let Some(item_id) = found_id else {
        return (1.0, true);
    };
    let last: Vec<lifescape_market::LastFill> = store
        .last_fills(item_id, lifescape_market::PRICE_FILLS as i64)
        .unwrap_or_default()
        .into_iter()
        .map(|(price, quantity)| lifescape_market::LastFill { price, quantity })
        .collect();
    let size = lifescape_market::impact_size(&last);
    let levels = |side| store.book_levels_covering(item_id, side, size).unwrap_or_default();
    let current = lifescape_market::current_price(&last, &levels(lifescape_store::OrderSide::Buy), &levels(lifescape_store::OrderSide::Sell));
    (current.price as f64, !current.traded)
}
