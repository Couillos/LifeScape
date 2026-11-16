#![forbid(unsafe_code)]
//! Time concession of a resting order toward its OWN limit (plan marché
//! M3/M4, user decisions Q2/Q4 of 2026-11-16). Replaces the uniform 5 %
//! step of `zip::demand_step` in the daemon's repricing:
//!
//! `p(t) = opening + (limit - opening) * (t / H) ^ (1 / beta)`
//!
//! - `t`: ticks since the curve started (the lineage's t0, restarted by
//!   an own fill), `H`: the motive's horizon (`need` 6 000, `gear`/
//!   `tool`/`surplus` 36 000, `stock`/`loot` 72 000 ticks);
//! - the limit is the order's own bound: a buyer's ceiling (what it may
//!   pay), a seller's floor (its own cost, outside option, keep value);
//! - `beta` (Boulware < 1 < Conceder): `concession_beta(price_patience,
//!   urgency)`, the bot's drawn trait times the motive's urgency.
//!
//! ZIP feedback kept: an own fill backs the price off one ZIP step and
//! restarts the curve from there (a seller that sold was cheap, a buyer
//! that bought paid too much); a public traded price on the conceding
//! side pulls the quote halfway toward it. Without a fill the price only
//! moves toward the limit (monotone) and never leaves [opening, limit].
//! An amend happens only past `AMEND_MIN_PCT` (1 %, at least one coin),
//! or to land exactly on the limit. A pressed order (blocking need, full
//! bank) crosses the spread when the opposite best is inside its limit
//! (M4), and any order takes the opposite best its curve would reach
//! within the next window. Q4: a seller at its limit after its horizon
//! with no fill withdraws; a buyer stays at its limit while its need
//! lives.

/// Smallest and largest shape: the bounds keep a curve from freezing
/// (beta -> 0) or jumping to the limit at once (beta -> inf).
pub const MIN_BETA: f64 = 0.25;
pub const MAX_BETA: f64 = 4.0;
/// Amend threshold: a move under 1 % of the current price (and under one
/// coin) waits for the next window (daemon CPU, plan §3.3).
pub const AMEND_MIN_PCT: i64 = 1;
/// ZIP back-off after an own fill, in percent of the price (at least one
/// coin): the old `demand_step` size, now once per fill only.
pub const ZIP_STEP_PCT: i64 = 5;
/// Share of the gap to a public traded price a quote closes per window
/// (ZIP learning rate on public fills).
pub const PUBLIC_PULL: f64 = 0.5;
/// Repricing window bounds (ticks): a horizon is walked in about
/// `WINDOWS_PER_HORIZON` windows, never faster than `MIN_WINDOW_TICKS`
/// (CPU) nor slower than `MAX_WINDOW_TICKS` (the old patience).
pub const WINDOWS_PER_HORIZON: u64 = 12;
pub const MIN_WINDOW_TICKS: u64 = 500;
pub const MAX_WINDOW_TICKS: u64 = 2000;

/// How badly the order's owner wants it through (motive and state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    /// Stock bought ahead, loot liquidation: concedes late (x 0.5).
    Patient,
    /// Gear, tools, surplus: the bot's own patience alone (x 1).
    Normal,
    /// Blocking need, full bank: concedes early (x 2) and crosses the
    /// spread inside its limit.
    Pressed,
}

impl Urgency {
    /// Multiplier on the patience shape.
    pub fn factor(self) -> f64 {
        match self {
            Urgency::Patient => 0.5,
            Urgency::Normal => 1.0,
            Urgency::Pressed => 2.0,
        }
    }
}

/// Curve shape from the bot's drawn `price_patience` (0..1, the life's
/// profile trait) and the motive's urgency: patience 0 -> 2 (concedes
/// early), 0.5 -> 1 (linear), 1 -> 0.5 (concedes late), times the
/// urgency factor, inside [MIN_BETA, MAX_BETA].
pub fn concession_beta(price_patience: f64, urgency: Urgency) -> f64 {
    let patience = if price_patience.is_finite() { price_patience.clamp(0.0, 1.0) } else { 0.5 };
    (2f64.powf(1.0 - 2.0 * patience) * urgency.factor()).clamp(MIN_BETA, MAX_BETA)
}

/// Repricing window of a horizon (ticks).
pub fn window_ticks(horizon_ticks: u64) -> u64 {
    (horizon_ticks / WINDOWS_PER_HORIZON).clamp(MIN_WINDOW_TICKS, MAX_WINDOW_TICKS)
}

/// Phase of an order's windows inside `[0, window)`: a stable hash of
/// its account and object, so orders adopted together (daemon restart)
/// never walk in one block (plan C6).
pub fn window_offset(account: &str, item: i64, window: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in account.bytes().chain(item.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash % window.max(1)
}

/// One concession curve: from `opening` to `limit` in `horizon_ticks`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Concession {
    pub opening: i64,
    pub limit: i64,
    pub horizon_ticks: u64,
    pub beta: f64,
}

impl Concession {
    /// Price `elapsed` ticks into the curve: inside [opening, limit]
    /// (either order), exactly the limit from the horizon on. The move
    /// is truncated toward the opening (never past the curve).
    pub fn price_at(&self, elapsed: u64) -> i64 {
        let opening = self.opening.max(1);
        let limit = self.limit.max(1);
        if self.horizon_ticks == 0 || elapsed >= self.horizon_ticks {
            return limit;
        }
        let beta = if self.beta.is_finite() { self.beta.clamp(MIN_BETA, MAX_BETA) } else { 1.0 };
        let fraction = (elapsed as f64 / self.horizon_ticks as f64).powf(1.0 / beta);
        let moved = ((limit - opening) as f64 * fraction).trunc() as i64;
        between(opening + moved, opening, limit)
    }
}

fn between(price: i64, one: i64, other: i64) -> i64 {
    price.clamp(one.min(other), one.max(other)).max(1)
}

/// What one window sees of a resting order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub sell: bool,
    /// Price resting now.
    pub current: i64,
    pub curve: Concession,
    /// Ticks since the curve started (lineage t0 or the last own fill).
    pub elapsed_ticks: u64,
    /// Ticks of one window (`window_ticks`): the look-ahead of a take.
    pub window_ticks: u64,
    /// The order filled (even partly) since the last window.
    pub filled: bool,
    /// Current traded price of the object (public prints), if traded.
    pub public_price: Option<i64>,
    /// The order's own last fill price (its ZIP back-off already read
    /// it): a public price equal to it is its own print, never a pull.
    pub own_fill: Option<i64>,
    /// Best opposite price of ANOTHER account: a seller's best bid, a
    /// buyer's best ask.
    pub best_opposite: Option<i64>,
    pub pressed: bool,
}

/// Decision of one window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Stay (inside the amend threshold, or parked at the limit).
    Hold,
    /// Amend to this price (it may cross: a take).
    Amend(i64),
    /// Own fill: back off to `opening` and restart the curve there.
    Restart { opening: i64 },
    /// Q4: a seller at its limit past its horizon with no buyer.
    Withdraw,
}

/// Toward the limit: the further of two prices (buyer up, seller down).
fn further(sell: bool, one: i64, other: i64) -> i64 {
    if sell {
        one.min(other)
    } else {
        one.max(other)
    }
}

/// True when `price` is inside the order's limit (a seller at or over
/// its floor, a buyer at or under its ceiling).
pub fn within_limit(sell: bool, price: i64, limit: i64) -> bool {
    if sell {
        price >= limit
    } else {
        price <= limit
    }
}

/// Smallest move worth an amend from `current`.
pub fn amend_threshold(current: i64) -> i64 {
    ((current.max(1) * AMEND_MIN_PCT + 99) / 100).max(1)
}

/// ZIP back-off of a filled order from `current` (one step away from
/// the counterparty, at least one coin), never past its limit for a
/// buyer (a seller's way up has no bound of its own).
pub fn zip_backoff(sell: bool, current: i64, limit: i64) -> i64 {
    let step = (current.max(1) * ZIP_STEP_PCT / 100).max(1);
    if sell {
        (current + step).max(limit).max(1)
    } else {
        (current - step).max(1).min(limit.max(1))
    }
}

/// One window of the order (pure).
pub fn step(window: &Window) -> Step {
    let sell = window.sell;
    let limit = window.curve.limit.max(1);
    let current = window.current.max(1);
    if window.filled {
        let opening = zip_backoff(sell, current, limit);
        return if opening == current { Step::Hold } else { Step::Restart { opening } };
    }
    // M4: the opposite best inside the limit is taken when pressed, or
    // when the curve reaches it within the next window anyway.
    if let Some(opposite) = window.best_opposite.filter(|price| *price >= 1 && within_limit(sell, *price, limit)) {
        let ahead = window.curve.price_at(window.elapsed_ticks.saturating_add(window.window_ticks.max(1)));
        let reached = if sell { ahead <= opposite } else { ahead >= opposite };
        if (window.pressed || reached) && opposite != current {
            return Step::Amend(opposite);
        }
    }
    let mut target = window.curve.price_at(window.elapsed_ticks);
    if let Some(public) = window.public_price.filter(|price| *price >= 1 && Some(*price) != window.own_fill) {
        // Others traded on the conceding side, inside the limit: pull
        // halfway toward that print (public data only, D22).
        let conceding = if sell { public < current } else { public > current };
        if conceding && within_limit(sell, public, limit) {
            let pulled = current + ((public - current) as f64 * PUBLIC_PULL).trunc() as i64;
            target = further(sell, target, pulled);
        }
    }
    // Monotone without a fill, never past the limit.
    target = further(sell, target, current);
    target = if sell { target.max(limit) } else { target.min(limit) };
    if target == current {
        let expired = window.elapsed_ticks >= window.curve.horizon_ticks;
        return if sell && current <= limit && expired { Step::Withdraw } else { Step::Hold };
    }
    if (target - current).abs() >= amend_threshold(current) || target == limit {
        Step::Amend(target)
    } else {
        Step::Hold
    }
}

#[cfg(test)]
#[path = "concession_tests.rs"]
mod tests;
