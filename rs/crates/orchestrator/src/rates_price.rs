#![forbid(unsafe_code)]
//! Sale-price estimate for the rates (PRD3 lot 2), split from
//! `rates_service` (rule 4): trades first, then the waiting buyers, then
//! the 1-coin default. NPC shop prices never value anything.

use lifescape_store::Store;

/// Estimated sale price of an item display name (PRD3 lot 2): 24 h
/// market VWAP, else the last print ever, else the best resting bid (a
/// buyer is waiting: that is what the item fetches now), else 1 coin.
/// The flag marks a price no trade backs yet (`price_prior`). NPC shop
/// prices never value anything: nobody sells to merchants.
pub fn price_estimate(store: &Store, item: &str) -> (f64, bool) {
    let catalogue = match store.list_items() {
        Ok(rows) => rows,
        Err(_) => return (1.0, true),
    };
    if item.eq_ignore_ascii_case("coins") {
        return (1.0, false);
    }
    let Some(found) = catalogue.iter().find(|row| row.name == item) else {
        return (1.0, true);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_secs() as i64)
        .unwrap_or(0);
    let prints = store.executions_for_item(found.item_id, 0, now + 1).unwrap_or_default();
    let recent: Vec<_> = prints.iter().filter(|print| print.clock >= now - 86400).collect();
    let volume: i64 = recent.iter().map(|print| print.quantity).sum();
    if volume > 0 {
        let turnover: i64 = recent.iter().map(|print| print.quantity * print.price).sum();
        return (turnover as f64 / volume as f64, false);
    }
    if let Some(last) = prints.iter().max_by_key(|print| print.clock) {
        return (last.price.max(1) as f64, false);
    }
    let best_bid = store
        .open_book(found.item_id, lifescape_store::OrderSide::Buy)
        .unwrap_or_default()
        .iter()
        .map(|order| order.price)
        .max();
    match best_bid {
        Some(bid) if bid >= 1 => (bid as f64, true),
        _ => (1.0, true),
    }
}
