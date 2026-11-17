//! Market value of a production lot (user requirement 2026-11-14: « le
//! brain doit être capable d'évaluer la rentabilité en fonction de toutes
//! les dépendances et des rates, pour tout le marché »). Pure helpers of
//! `bid_response`, public data and the bot's own state only:
//! - where a production leg happens for this bot and what reaching it
//!   costs: walk ticks from where its output goes (the next step's site,
//!   the Varrock fountain for the finished lot), toll and boat fares
//!   (`leg_site`, `walk_ticks`); never a fixed round trip per answer;
//! - what the market absorbs of a lot (`absorbed_lot`): others' bids
//!   above the unit cost, sales at the current price over a day (half the
//!   24 h volume, minus the asks already resting at or under it), the
//!   bot's own alchemy floor; minus supply on its way (asks just above the
//!   best bid), units other producers publicly claimed and the bot's own
//!   free stock; the bot takes its share (`PRODUCER_SPLIT`);
//! - the share of a training a lot carries (`training_share`): none when
//!   the bot's ambition climbs that skill anyway, else one lot of the
//!   `TRAINING_AMORTIZE_LOTS` lots the level serves.

use crate::market_fit::BookView;

/// Where every lot is delivered (deals hand over around the fountain).
pub const VARROCK_FOUNTAIN: (i64, i64, i64) = (3213, 3429, 0);
/// Ticks per tile of Manhattan distance (a walk with its detours).
pub const WALK_TICKS_PER_TILE: f64 = 1.0;
/// One handover at the fountain (convocation wait, trade screens).
pub const HANDOVER_TICKS: f64 = 100.0;
/// Share of a day's traded volume a new seller places at the current
/// price within the lot's horizon.
pub const SALES_SHARE_PCT: i64 = 50;
/// Asks this far above the best bid count as supply on its way.
pub const PENDING_SUPPLY_PCT: i64 = 125;
/// A level trained for a lot serves this many lots of the same tier
/// (the products made while training sell on top).
pub const TRAINING_AMORTIZE_LOTS: f64 = 4.0;
/// Units the bot alchs past what the market absorbs (one inventory).
pub const ALCH_LOT: i64 = crate::market_fit::SALE_LOT_MIN;

/// Ticks to walk from `from` to `to` (surface tiles, Manhattan).
pub fn walk_ticks(from: (i64, i64, i64), to: (i64, i64, i64)) -> f64 {
    let (from_x, from_z, _) = lifescape_gamedata::access::surface_of(from.0, from.1, from.2);
    let (to_x, to_z, _) = lifescape_gamedata::access::surface_of(to.0, to.1, to.2);
    ((from_x - to_x).abs() + (from_z - to_z).abs()) as f64 * WALK_TICKS_PER_TILE
}

/// Where one leg happens for this bot and the coins of the trip there
/// and back (toll unless Prince Ali Rescue is done, Karamja boat).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegSite {
    pub site: (i64, i64, i64),
    pub fare: f64,
}

/// The site the brain would pick for `method` seen from `dest` (the
/// nearest safe one at the bot's combat level, `combat::pick_site_gated`);
/// a method that works anywhere happens at `dest`. `None`: no site is
/// safe or open for this bot (guild door, key, combat level).
pub fn leg_site(method: &str, dest: (i64, i64, i64), combat: u8, free_pass: bool) -> Option<LegSite> {
    use crate::combat::{pick_site_gated, Fare, Pass, SitePick};
    let fare = Fare { wallet: i64::MAX / 4, free_pass };
    let site = match pick_site_gated(method, combat, Some(dest), fare, &Pass::default()) {
        SitePick::Anywhere => return Some(LegSite { site: dest, fare: 0.0 }),
        SitePick::At(site) => return Some(LegSite { site, fare: 0.0 }),
        SitePick::Tolled(site) | SitePick::Boat { site, .. } => site,
        SitePick::Fare(_) | SitePick::Needs(_) | SitePick::Access(_, _) | SitePick::Cooled => return None,
    };
    Some(LegSite { site, fare: fares(dest, site, free_pass) })
}

/// Coins of a round trip from `from` to `site`: the toll both ways when
/// it crosses the fence (free after Prince Ali Rescue), the boat both
/// ways for Karamja.
fn fares(from: (i64, i64, i64), site: (i64, i64, i64), free_pass: bool) -> f64 {
    let toll = if !free_pass && lifescape_gamedata::toll::crossing(from, site) { 2 * lifescape_gamedata::toll::FARE } else { 0 };
    let boat = if lifescape_gamedata::access::behind_boat(site.0, site.1, site.2) { 2 * lifescape_gamedata::services::BOAT_FARE_GP } else { 0 };
    (toll + boat) as f64
}

/// What the market takes of one lot at a unit cost (coins, time
/// included).
#[derive(Debug, Clone, PartialEq)]
pub struct AbsorbedLot {
    /// Units this bot makes.
    pub count: i64,
    /// Coins those units bring (best prices first).
    pub revenue: f64,
    /// Units sold into resting bids, at the current price, alched.
    pub from_bids: i64,
    pub from_sales: i64,
    pub from_alch: i64,
}

/// The lot one producer makes of an object whose unit costs `cost`
/// (module doc). `held` is the bot's free stock (it sells before making),
/// `alch_net` the coins one cast of its own brings net of runes and
/// casting time (`None`: not a caster). `None` when nothing pays.
pub fn absorbed_lot(view: &BookView, cost: f64, held: i64, alch_net: Option<f64>, split: i64, max_lot: i64) -> Option<AbsorbedLot> {
    let cap = (max_lot * split.max(1)).max(1);
    // Bids that pay more than the unit cost, best first.
    let mut bids = view.bids.clone();
    bids.sort_by(|left, right| right.0.cmp(&left.0));
    let mut ladder: Vec<(f64, u8)> = Vec::new();
    for (price, qty) in bids.iter().filter(|(price, _)| *price as f64 > cost) {
        let room = cap - ladder.len() as i64;
        ladder.extend(std::iter::repeat_n((*price as f64, 0u8), (*qty).max(0).min(room.max(0)) as usize));
    }
    // Supply on its way: asks just above the best bid walk down into it.
    let pending = view.best_bid().map(|best| view.asks_at_or_under(best.saturating_mul(PENDING_SUPPLY_PCT) / 100)).unwrap_or(0);
    let bid_units = (ladder.len() as i64 - pending).max(0);
    ladder.truncate(bid_units as usize);
    // Sales at the current price over the horizon, behind the asks that
    // already rest at or under it.
    if let Some(price) = view.current.filter(|price| *price as f64 > cost) {
        let capacity = (view.volume_24h.max(0) * SALES_SHARE_PCT / 100 - view.asks_at_or_under(price)).clamp(0, cap);
        ladder.extend(std::iter::repeat_n((price as f64, 1u8), capacity as usize));
    }
    ladder.sort_by(|left, right| right.0.total_cmp(&left.0));
    let open = ladder.len() as i64 - held.max(0) - view.claimed.max(0);
    let market_count = if open > 0 { ((open + split - 1) / split.max(1)).clamp(1, max_lot) } else { 0 };
    let alch = alch_net.filter(|net| *net > cost);
    let count = match alch {
        Some(_) => market_count.max(ALCH_LOT.min(max_lot)),
        None => market_count,
    };
    if count <= 0 {
        return None;
    }
    let mut lot = AbsorbedLot { count, revenue: 0.0, from_bids: 0, from_sales: 0, from_alch: 0 };
    for unit in 0..count as usize {
        let market = ladder.get(unit).filter(|_| (unit as i64) < market_count).copied();
        match (market, alch) {
            (Some((price, _)), Some(net)) if net > price => {
                lot.revenue += net;
                lot.from_alch += 1;
            }
            (Some((price, source)), _) => {
                lot.revenue += price;
                if source == 0 {
                    lot.from_bids += 1;
                } else {
                    lot.from_sales += 1;
                }
            }
            (None, Some(net)) => {
                lot.revenue += net;
                lot.from_alch += 1;
            }
            (None, None) => {}
        }
    }
    Some(lot)
}

/// Share of a training's net cost one lot carries: none when the bot's
/// own ambition climbs the skill to `to` anyway (`planned` ≥ `to`), else
/// one lot of the `TRAINING_AMORTIZE_LOTS` the level serves.
pub fn training_share(planned: u8, to: u8) -> f64 {
    if planned >= to {
        0.0
    } else {
        1.0 / TRAINING_AMORTIZE_LOTS
    }
}

#[cfg(test)]
#[path = "market_value_tests.rs"]
mod tests;
