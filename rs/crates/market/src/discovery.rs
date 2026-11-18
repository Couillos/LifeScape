#![forbid(unsafe_code)]
//! Price discovery of an object NOBODY traded yet (plan marché M7,
//! §3.8). Its first orders open on PUBLIC anchors only (D22/D11: nothing
//! hidden), never on a made-up price:
//! - what an F2P NPC shop pays for one unit (Q3, server
//!   `calc_shop_value`): a seller's outside option;
//! - the net of one alchemy cast (high or low, what the caster gets, its
//!   runes at their current price): a caster's outside option;
//! - the production cost the guides publish (cheapest recipe, chain or
//!   100 % market line, inputs at their current price);
//! - the price an F2P shop sells it at: no buyer pays more on the market.
//!
//! Rules: a seller's floor is `max(own keep value, shop sale, alch net)`;
//! a seller opens ABOVE the production cost when it is known
//! (`cost × (1 + margin)`), never over the shop's selling price, never
//! under its floor; a buyer opens UNDER its use value (`value × (1 −
//! margin)`, the published cost or the shop's price when it has none of
//! its own) and never over the published cost, at least at the sellers'
//! public floor (a lower bid only
//! sends them to the shop), never over its ceiling. The margin is the
//! bot's profile margin (`margin_of`, from its price patience). Without
//! any anchor the caller's own rule stands (the order keeps the price the
//! bot chose). Once the first fill prints, the object is traded: the
//! current price, the concession curve and the ZIP back-off take over.

/// The public anchors of one object (`None`: unknown, or not offered).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchors {
    /// Best unit price an F2P shop open to every bot pays (Q3).
    pub shop_sale: Option<i64>,
    /// Net coins of one alchemy cast on a unit, runes at current prices.
    pub alch_net: Option<i64>,
    /// Production cost of one unit the guides publish.
    pub make_cost: Option<i64>,
    /// Cheapest F2P shop selling price of one unit.
    pub shop_buy: Option<i64>,
}

fn positive(value: Option<i64>) -> Option<i64> {
    value.filter(|coins| *coins >= 1)
}

impl Anchors {
    /// True when at least one anchor is known.
    pub fn known(&self) -> bool {
        [self.shop_sale, self.alch_net, self.make_cost, self.shop_buy].into_iter().any(|anchor| positive(anchor).is_some())
    }

    /// The sellers' public floor: the best outside option (shop sale,
    /// alchemy net), 0 when none.
    pub fn floor(&self) -> i64 {
        positive(self.shop_sale).unwrap_or(0).max(positive(self.alch_net).unwrap_or(0))
    }
}

/// Profile margin of a bot of price patience `patience` (the planner's
/// `Profile::margin`: 5 % for the most hurried, 20 % for the most
/// patient).
pub fn margin_of(patience: f64) -> f64 {
    0.05 + patience.clamp(0.0, 1.0) * 0.15
}

/// Share of an alchemy cast's net an overstocked seller leaves the
/// caster (user 2026-11-18, point 4).
pub const ALCH_OUTLET_MARGIN_PCT: i64 = 10;

/// What the alchemy outlet pays per unit (user 2026-11-18, point 4,
/// universal): the cast's net (`net`: alchemy value less the runes) less
/// the caster's `ALCH_OUTLET_MARGIN_PCT`; `None` when nothing is left
/// (a cast that loses is no outlet).
pub fn alch_outlet_price(net: i64) -> Option<i64> {
    let price = net.saturating_mul(100 - ALCH_OUTLET_MARGIN_PCT) / 100;
    (price >= 1).then_some(price)
}

/// Plan prix §2 (user 2026-11-18): opening of a listing over its floor,
/// `floor x (1 + margin)` rounded up, at least one coin above: the
/// concession curve always has a course.
pub fn open_over_floor(floor: i64, margin: f64) -> i64 {
    let floor = floor.max(1);
    let raised = (floor as f64 * (1.0 + margin.max(0.0)) - 1e-9).ceil() as i64;
    raised.max(floor + 1)
}

/// Plan prix §2: opening of a standing bid under its ceiling, `ceiling x
/// (1 - margin)` rounded down, at least one coin under, never under 1.
pub fn bid_under_ceiling(ceiling: i64, margin: f64) -> i64 {
    let ceiling = ceiling.max(1);
    let lowered = (ceiling as f64 * (1.0 - margin.clamp(0.0, 1.0)) + 1e-9).floor() as i64;
    lowered.min(ceiling - 1).max(1)
}

/// Plan prix §1, Q1 (user 2026-11-18): drop of a seller's floor per
/// horizon that ended unsold.
pub const FLOOR_DECAY_PCT: i64 = 10;

/// Horizons that ended unsold, net of the sales since (each sale takes
/// one back: the floor climbs a notch).
pub fn unsold_horizons(withdrawals: i64, sales: i64) -> i64 {
    (withdrawals - sales).max(0)
}

/// Plan prix §1, Q2 (user 2026-11-18: under the making cost when nothing
/// sells): the floor after `unsold` horizons, `keep x 0.9^unsold`, never
/// under `bound` (the outside option the stock can still reach: the
/// alchemy outlet, 1 coin) nor over `keep`.
pub fn learned_floor(keep: i64, bound: i64, unsold: i64) -> i64 {
    let keep = keep.max(1);
    let factor = (1.0 - FLOOR_DECAY_PCT as f64 / 100.0).powi(unsold.clamp(0, 64) as i32);
    let decayed = (keep as f64 * factor).floor() as i64;
    decayed.max(bound.max(1)).min(keep)
}

/// A seller's floor: its own keep value, never under the public outside
/// options, at least 1 coin.
pub fn seller_floor(keep: i64, anchors: &Anchors) -> i64 {
    keep.max(anchors.floor()).max(1)
}

/// Opening ask of a never-traded object: over the published production
/// cost (`cost × (1 + margin)`, rounded up) when known, else `fallback`
/// (the seller's own rule); never over the shop's selling price, never
/// under `floor`.
pub fn ask_open(anchors: &Anchors, floor: i64, margin: f64, fallback: i64) -> i64 {
    let base = match positive(anchors.make_cost) {
        Some(cost) => (cost as f64 * (1.0 + margin.max(0.0)) - 1e-9).ceil() as i64,
        None => fallback,
    };
    let capped = positive(anchors.shop_buy).map_or(base, |shop| base.min(shop));
    capped.max(floor).max(1)
}

/// Opening bid of a never-traded object (`None`: no anchor, the
/// buyer's own rule stands): under the use value (`value`, else the
/// published cost, else the shop's selling price) by the margin and
/// never over the published cost (a producer selling at its cost takes
/// it: the concession pays more only if nobody does), at least the
/// sellers' public floor, never over the shop's selling price nor the
/// `ceiling`.
pub fn bid_open(anchors: &Anchors, value: Option<i64>, ceiling: i64, margin: f64) -> Option<i64> {
    if !anchors.known() {
        return None;
    }
    let worth = positive(value).or(positive(anchors.make_cost)).or(positive(anchors.shop_buy));
    let under = worth.map_or(0, |worth| (worth as f64 * (1.0 - margin.clamp(0.0, 1.0)) + 1e-9).floor() as i64);
    let under = positive(anchors.make_cost).map_or(under, |cost| under.min(cost));
    let opened = under.max(anchors.floor());
    let capped = positive(anchors.shop_buy).map_or(opened, |shop| opened.min(shop));
    Some(capped.min(ceiling.max(1)).max(1))
}

/// Opening and floor of a listing of MADE stock (`zip::seller_open`):
/// a traded object, or one without anchors, keeps that rule on the
/// anchored floor; a never-traded one opens at `ask_open` (its fallback:
/// twice the floor), or into a richer bid.
pub fn listing_open(reference: crate::price::Reference, best_bid: Option<i64>, keep: i64, last_sell: Option<crate::zip::OwnFill>, anchors: &Anchors, margin: f64) -> (i64, i64) {
    let floor = seller_floor(keep, anchors);
    let (open, floor) = crate::zip::seller_open(reference, best_bid, floor, last_sell);
    if reference.traded || !anchors.known() {
        return (open, floor);
    }
    let fallback = floor.saturating_mul(crate::zip::UNTRADED_OPEN_RESERVES);
    (ask_open(anchors, floor, margin, fallback).max(best_bid.unwrap_or(0)), floor)
}

/// The empty-book anchor of FOUND stock (`zip::found_open`): on a
/// never-traded object with anchors, `ask_open` with the caller's anchor
/// (its high alchemy value), else twice the floor, as fallback.
pub fn loot_anchor(reference: crate::price::Reference, anchor: Option<i64>, floor: i64, anchors: &Anchors, margin: f64) -> Option<i64> {
    if reference.traded || !anchors.known() {
        return anchor;
    }
    let fallback = positive(anchor).unwrap_or(floor.saturating_mul(crate::zip::UNTRADED_OPEN_RESERVES));
    Some(ask_open(anchors, floor, margin, fallback))
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
