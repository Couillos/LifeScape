#![forbid(unsafe_code)]
//! Sale-price estimate for the rates (PRD3 lot 2), split from
//! `rates_service` (rule 4): trades first, then the waiting buyers, then
//! the 1-coin default. NPC shop prices never value anything.

use lifescape_store::Store;

/// Estimated sale price of an item display name: its current price
/// (user 2026-11-14, `lifescape_market::current_price`: median of the
/// last five fills, any age, clamped by the impact prices of the
/// resting book; never traded: the book, else 1 coin). The flag marks a
/// price no trade backs yet (`price_prior`). NPC shop prices never value
/// anything: nobody sells to merchants.
///
/// One source (2026-11-23): inside the daemon the market's memory answers
/// (`price_estimate_memory`: the fills ring, the live books, no SQL read,
/// the same `current_price_at`); `store` is read only when the market
/// service is not running (a separate CLI command, a test).
pub fn price_estimate(store: &Store, item: &str) -> (f64, bool) {
    match crate::market_service::shared() {
        Some(service) => price_estimate_memory(&service, item, crate::market_service::MarketService::now_clock()),
        None => price_estimate_sql(store, item, crate::market_service::MarketService::now_clock()),
    }
}

/// `price_estimate` for a caller that owns no store (the launch pass):
/// the market's memory inside the daemon, else the pooled connection of
/// this thread (`store_pool`, never a fresh open per call). `None`: no
/// service and the store cannot be read.
pub fn price_of(item: &str) -> Option<(f64, bool)> {
    price_of_in(crate::market_service::shared().as_deref(), item)
}

/// `price_of` on the market handle the caller holds (the brain's
/// `BrainDeps`): same answer, no read of the process-wide registry.
pub fn price_of_in(market: Option<&crate::market_service::MarketService>, item: &str) -> Option<(f64, bool)> {
    if let Some(service) = market {
        return Some(price_estimate_memory(service, item, crate::market_service::MarketService::now_clock()));
    }
    let store = crate::store_pool::open(&crate::market_service::db_path()).ok()?;
    Some(price_estimate_sql(&store, item, crate::market_service::MarketService::now_clock()))
}

/// Memory path: name lookup in the items table loaded by the service,
/// `reference_lot` (exact, never the rounded per-unit `reference`).
pub fn price_estimate_memory(service: &crate::market_service::MarketService, item: &str, now: i64) -> (f64, bool) {
    if item.eq_ignore_ascii_case("coins") {
        return (1.0, false);
    }
    let Some(item_id) = service.item_id_by_name(item) else {
        return (1.0, true);
    };
    let current = service.reference_lot_at(item_id, now);
    // Per unit, exact (plan prix lot D).
    (current.price as f64 / crate::market_units::lot_of(item_id) as f64, !current.traded)
}

/// Store path: five indexed reads (name, last five fills, the best bids
/// then the best asks until the impact size is covered, newest fill),
/// never the item's whole history.
pub(crate) fn price_estimate_sql(store: &Store, item: &str, now: i64) -> (f64, bool) {
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
    let newest = store.newest_fill_clock(item_id).ok().flatten();
    let current = lifescape_market::current_price_at(&last, &levels(lifescape_store::OrderSide::Buy), &levels(lifescape_store::OrderSide::Sell), newest, now);
    // Per unit, exact (plan prix lot D).
    (current.price as f64 / crate::market_units::lot_of(item_id) as f64, !current.traded)
}
