#![forbid(unsafe_code)]
//! Order repricing: prices follow supply and demand (user rule
//! 2026-11-13: 1 coin is only the bootstrap). An order placed with
//! bounds takes one `lifescape_market::demand_step` (5%, at least one
//! coin) per patience window, by amend (cancel + replace: a fresh price
//! may cross the book), its direction read from the window's fills:
//! - an ask that sold (even partly) walks UP, toward its cap (none of
//!   its own: the 5x band of a traded reference bounds it);
//! - an ask that rested unfilled walks down to its reserve floor;
//! - a bid that rested unfilled walks up to its ceiling (the wallet
//!   share it may spend);
//! - a bid that filled walks down to its opening price.
//!
//! Partly filled orders (`matched`) keep walking. A wanted, scarce object
//! gets bid up until a producer finds it worth making; one that sells
//! fast climbs; an abundant one drifts down to what sellers accept.
//!
//! Bounds live in the market engine's memory: after a daemon restart the
//! brain re-adopts its resting listings (`adopt`) and its standing bids
//! (`keep_bid`); until then an order keeps its last price.

use lifescape_json::{object, str_value, Value};

use crate::market_service::MarketService;

/// Default patience: ticks of one window (one step at most per window).
pub const PATIENCE_TICKS: u64 = 200;
/// A sell's own cap: none (the 5x band of a traded reference bounds the
/// walk-up, see `walk_cap`).
pub const SELL_CAP: i64 = i64::MAX / 4;

/// Repricing bounds of one resting order.
#[derive(Clone, Debug)]
pub struct QuoteBounds {
    pub account: String,
    pub sell: bool,
    pub floor: i64,
    pub ceiling: i64,
    pub patience_ticks: u64,
    /// Daemon wall seconds of the last window close (price change or a
    /// window spent at a bound).
    pub last_change_wall: f64,
    /// Units still open when the window started: fewer now = it filled.
    pub last_remaining: i64,
}

fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

/// Registers bounds for a resting order (called by `place`).
pub fn register(service: &MarketService, order: i64, bounds: QuoteBounds) {
    service.lock_engine().quote_bounds.insert(order, bounds);
}

/// Bounds of a resting order, if it reprices.
pub fn bounds_of(service: &MarketService, order: i64) -> Option<QuoteBounds> {
    service.lock_engine().quote_bounds.get(&order).cloned()
}

/// Bounds parsed from a `place` request: `ceiling` (buy: the most it
/// pays; sell: an optional cap) or `floor` (sell reserve), optional
/// `patience` ticks. `None` = fixed-price order. A buy never walks under
/// its opening price, a sell never over its cap.
pub fn bounds_from_request(request: &Value, account: &str, sell: bool, price: i64) -> Option<QuoteBounds> {
    let ceiling = request.get("ceiling").as_i64();
    let floor = request.get("floor").as_i64();
    if ceiling.is_none() && floor.is_none() {
        return None;
    }
    let (floor, ceiling) = if sell {
        (floor.unwrap_or(1).clamp(1, price.max(1)), ceiling.unwrap_or(SELL_CAP).max(price))
    } else {
        (price, ceiling.unwrap_or(price).max(price))
    };
    Some(QuoteBounds {
        account: account.to_string(),
        sell,
        floor,
        ceiling,
        patience_ticks: request.get("patience").as_i64().map(|ticks| ticks.max(1) as u64).unwrap_or(PATIENCE_TICKS),
        last_change_wall: wall_now(),
        last_remaining: request.get("qty").as_i64().unwrap_or(0),
    })
}

/// Highest price a step may reach: the order's own ceiling, and for a
/// traded object the 5x guard band (an amend past it would be refused).
fn walk_cap(service: &MarketService, item: i64, bounds: &QuoteBounds) -> i64 {
    let reference = service.reference(item);
    if reference.traded {
        bounds.ceiling.min(reference.price.saturating_mul(5)).max(bounds.floor)
    } else {
        bounds.ceiling
    }
}

fn resting(row: &lifescape_store::Order) -> bool {
    matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched) && row.remaining > 0
}

/// One repricing pass at daemon wall `wall` with the measured tick
/// length. Returns the number of orders moved.
pub fn reprice_round(service: &MarketService, wall: f64, tick_ms: f64) -> usize {
    let entries: Vec<(i64, QuoteBounds)> =
        service.lock_engine().quote_bounds.iter().map(|(order, bounds)| (*order, bounds.clone())).collect();
    let mut moved = 0;
    for (order, bounds) in entries {
        let row = service.lock_store().get_order(order).ok().flatten();
        let Some(row) = row.filter(resting) else {
            service.lock_engine().quote_bounds.remove(&order);
            continue;
        };
        let patience_secs = bounds.patience_ticks as f64 * tick_ms.max(1.0) / 1000.0;
        if wall - bounds.last_change_wall < patience_secs {
            continue;
        }
        let filled = row.remaining < bounds.last_remaining;
        let cap = walk_cap(service, row.item_id, &bounds);
        let next = lifescape_market::demand_step(row.price, bounds.sell, filled, bounds.floor, cap);
        let window = QuoteBounds { last_change_wall: wall, last_remaining: row.remaining, ..bounds.clone() };
        if next == row.price {
            // Parked at a bound: the window closes, the next one reads
            // fresh fills.
            service.lock_engine().quote_bounds.insert(order, window);
            continue;
        }
        let reply = service.handle(&object(vec![
            ("op", str_value("amend")),
            ("account", str_value(&bounds.account)),
            ("order", Value::Int(order)),
            ("price", Value::Int(next)),
        ]));
        let mut engine = service.lock_engine();
        let guard = &mut engine.quote_bounds;
        if reply.get("ok").as_bool() == Some(true) {
            guard.remove(&order);
            if let Some(replacement) = reply.get("order").as_i64() {
                guard.insert(replacement, window);
            }
            moved += 1;
            crate::logln!(
                "market-reprice: {} order {order} {} -> {next} ({})",
                bounds.account,
                row.price,
                if filled { "filled" } else { "idle" }
            );
        } else {
            // The original rests untouched (the checks run before the
            // cancel): it keeps walking next window.
            guard.insert(order, window);
            crate::logln!("market-reprice: {} order {order} amend refused: {:?}", bounds.account, reply.get("error").as_str());
        }
    }
    moved
}

/// Re-adopts a resting order the engine has no bounds for (daemon
/// restart, listings placed before demand pricing): it walks from now.
/// Returns false when the order already walks.
pub fn adopt(service: &MarketService, order: i64, bounds: QuoteBounds) -> bool {
    let mut engine = service.lock_engine();
    if engine.quote_bounds.contains_key(&order) {
        return false;
    }
    engine.quote_bounds.insert(order, bounds);
    true
}

/// Outcome of `keep_bid`.
#[derive(Debug, Clone, PartialEq)]
pub enum StandingBid {
    /// A new bid was placed at this price.
    Placed { order: i64, price: i64 },
    /// The resting bid keeps its walk; its ceiling follows the wallet.
    Kept { order: i64, price: i64 },
}

/// A standing bid (D-B: it waits for a producer) of `account` for `qty`
/// of `item` with the per-unit cap `ceiling`. A resting bid keeps its
/// price and walk (re-placing it every pass reset the walk to the
/// reference: live 2026-11-13, wishes never climbed past a few coins);
/// only its ceiling follows the wallet, and a price over a shrunk
/// ceiling is amended down to it. Otherwise a bid opens at the covered
/// ask or the reference (`opening_bid`).
pub fn keep_bid(service: &MarketService, account: &str, item: i64, qty: i64, ceiling: i64) -> Result<StandingBid, String> {
    let ceiling = ceiling.max(1);
    let open = service.lock_store().account_orders(account).unwrap_or_default().into_iter().find(|row| {
        row.item_id == item && row.side == lifescape_store::OrderSide::Buy && resting(row)
    });
    if let Some(row) = open {
        let mut order = row.order_id;
        let mut price = row.price;
        if price > ceiling {
            let reply = service.handle(&object(vec![
                ("op", str_value("amend")),
                ("account", str_value(account)),
                ("order", Value::Int(order)),
                ("price", Value::Int(ceiling)),
            ]));
            if reply.get("ok").as_bool() != Some(true) {
                return Err(reply.get("error").as_str().unwrap_or("amend refused").to_string());
            }
            service.lock_engine().quote_bounds.remove(&order);
            order = reply.get("order").as_i64().unwrap_or(order);
            price = ceiling;
        }
        let mut engine = service.lock_engine();
        let bounds = engine.quote_bounds.entry(order).or_insert_with(|| QuoteBounds {
            account: account.to_string(),
            sell: false,
            floor: price,
            ceiling,
            patience_ticks: PATIENCE_TICKS,
            last_change_wall: wall_now(),
            last_remaining: row.remaining,
        });
        bounds.floor = bounds.floor.min(ceiling);
        bounds.ceiling = ceiling;
        return Ok(StandingBid::Kept { order, price });
    }
    let reference = service.reference(item);
    let (_, ask) = service.lock_engine().books.get(&item).map(|book| book.best()).unwrap_or((None, None));
    let price = lifescape_market::opening_bid(reference.price, ask, ceiling);
    let reply = service.handle(&object(vec![
        ("op", str_value("place")),
        ("account", str_value(account)),
        ("side", str_value("buy")),
        ("item", Value::Int(item)),
        ("qty", Value::Int(qty.max(1))),
        ("price", Value::Int(price)),
        ("ceiling", Value::Int(ceiling.max(price))),
    ]));
    if reply.get("ok").as_bool() != Some(true) {
        return Err(reply.get("error").as_str().unwrap_or("place refused").to_string());
    }
    Ok(StandingBid::Placed { order: reply.get("order").as_i64().unwrap_or(-1), price })
}

/// Registered order count (dashboard and tests).
pub fn tracked(service: &MarketService) -> usize {
    service.lock_engine().quote_bounds.len()
}

#[cfg(test)]
#[path = "market_reprice_tests.rs"]
mod tests;
