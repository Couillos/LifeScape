#![forbid(unsafe_code)]
//! Order repricing (PRD3 lot 2). An order placed with bounds walks
//! toward the other side while it rests unfilled: a buyer from the
//! reference up to its ceiling (what it can and will pay), a seller from
//! its asking price down to its floor (reserve). One proportional step
//! (`lifescape_market::step_toward`: 5%, at least one coin) per patience
//! window, by amend (cancel + replace: a fresh price may cross the book).
//!
//! With every untraded object at 1 coin, this is how prices form: a
//! wanted, scarce object gets bid up until a producer finds it worth
//! making; an abundant one drifts down to what sellers accept.
//!
//! Bounds live in memory only: after a daemon restart resting orders
//! keep their last price (no walk), which never breaks a deal.



use lifescape_json::{object, str_value, Value};

use crate::market_service::MarketService;

/// Default patience: ticks an order rests unfilled before one step.
pub const PATIENCE_TICKS: u64 = 200;

/// Repricing bounds of one resting order.
#[derive(Clone, Debug)]
pub struct QuoteBounds {
    pub account: String,
    pub sell: bool,
    pub floor: i64,
    pub ceiling: i64,
    pub patience_ticks: u64,
    /// Daemon wall seconds of the last price change.
    pub last_change_wall: f64,
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

/// Bounds parsed from a `place` request: `ceiling` (buy) or `floor`
/// (sell), optional `patience` ticks. `None` = fixed-price order.
pub fn bounds_from_request(request: &Value, account: &str, sell: bool, price: i64) -> Option<QuoteBounds> {
    let ceiling = request.get("ceiling").as_i64();
    let floor = request.get("floor").as_i64();
    if ceiling.is_none() && floor.is_none() {
        return None;
    }
    let (floor, ceiling) = if sell { (floor.unwrap_or(1).clamp(1, price.max(1)), price) } else { (price, ceiling.unwrap_or(price).max(price)) };
    Some(QuoteBounds {
        account: account.to_string(),
        sell,
        floor,
        ceiling,
        patience_ticks: request.get("patience").as_i64().map(|ticks| ticks.max(1) as u64).unwrap_or(PATIENCE_TICKS),
        last_change_wall: wall_now(),
    })
}

/// One repricing pass at daemon wall `wall` with the measured tick
/// length. Returns the number of orders moved.
pub fn reprice_round(service: &MarketService, wall: f64, tick_ms: f64) -> usize {
    let entries: Vec<(i64, QuoteBounds)> =
        service.lock_engine().quote_bounds.iter().map(|(order, bounds)| (*order, bounds.clone())).collect();
    let mut moved = 0;
    for (order, bounds) in entries {
        let row = service.lock_store().get_order(order).ok().flatten();
        // Partly filled orders (`matched`) keep walking their remainder.
        let resting = |row: &lifescape_store::Order| {
            row.remaining > 0 && matches!(row.status, lifescape_store::OrderStatus::Open | lifescape_store::OrderStatus::Matched)
        };
        let Some(row) = row.filter(resting) else {
            service.lock_engine().quote_bounds.remove(&order);
            continue;
        };
        let patience_secs = bounds.patience_ticks as f64 * tick_ms.max(1.0) / 1000.0;
        if wall - bounds.last_change_wall < patience_secs {
            continue;
        }
        let next = lifescape_market::step_toward(row.price, bounds.sell, bounds.floor, bounds.ceiling);
        if next == row.price {
            continue; // parked at its bound: the order rests as is
        }
        let reply = service.handle(&object(vec![
            ("op", str_value("amend")),
            ("account", str_value(&bounds.account)),
            ("order", Value::Int(order)),
            ("price", Value::Int(next)),
        ]));
        let mut engine = service.lock_engine();
        let guard = &mut engine.quote_bounds;
        guard.remove(&order);
        if reply.get("ok").as_bool() == Some(true) {
            if let Some(replacement) = reply.get("order").as_i64() {
                guard.insert(replacement, QuoteBounds { last_change_wall: wall, ..bounds.clone() });
            }
            moved += 1;
            crate::logln!("market-reprice: {} order {order} {} -> {next}", bounds.account, row.price);
        } else {
            crate::logln!("market-reprice: {} order {order} amend refused: {:?}", bounds.account, reply.get("error").as_str());
        }
    }
    moved
}

/// Registered order count (dashboard and tests).
pub fn tracked(service: &MarketService) -> usize {
    service.lock_engine().quote_bounds.len()
}
