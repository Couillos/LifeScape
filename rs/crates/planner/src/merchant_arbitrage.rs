//! Merchant arbitrage, open to every bot (plan équilibre step 5, user
//! decision D2, 2026-11-19): « tout bot, teneurs compris, peut acheter au
//! marché sous ce qu'un marchand ou l'alchimie lui rapporte (net du
//! trajet au taux du point 1) et revendre au marchand : le prix du marché
//! remonte vers l'exutoire par le jeu des joueurs, aucune règle de
//! plancher ». Live constat: yew logs rest at 1 coin on the market while
//! a general store pays 16 a log at its floor, without limit
//! (`shop.rs2` `calc_shop_value`: 10 % of the value once the shop holds
//! ten over its base).
//!
//! Pure: the public asks of one object (others' resting sells, cheapest
//! first), its outlets (each NPC shop that buys it, valued unit by unit
//! at the shop's likely stock — the fleet's recent sales and the lots in
//! flight, `orchestrator::brain_npc_sale` —; high alchemy for a caster,
//! runes paid), the bot's time value (what its work really earns per
//! tick, D1) and its free coins come in as arguments.
//!
//! Lot (`best_lot`): through one outlet, unit by unit from the cheapest
//! ask, a unit enters while what the outlet pays for it (the shop's
//! price falls with every unit it already holds) beats its ask plus its
//! own handling ticks at the bot's rate; asks only rise and the outlet
//! only falls, so the first unit that does not pay ends the lot. The
//! lot then pays once for its trip (the fountain handover, the walk to
//! the keeper and back) at the bot's rate; it is offered when it still
//! nets `BID_MIN_PROFIT` coins: the rate of its ticks then beats the
//! bot's own (no margin on top, the producers' rule). Its bid opens at
//! the dearest ask it takes; its ceiling is what the outlet pays for the
//! lot's last unit net of its handling and its share of the trip
//! (`net_outlet`): an unfilled bid climbs 10 % a pass toward it (the
//! market's walk), the market price climbs toward the outlet.
//!
//! Market makers (`net_outlet`): what one unit nets through an outlet
//! when a whole lot of `units` goes there (the last unit's value, the
//! trip shared over the units): the maker's reference value includes it
//! (`lifescape_market::maker`), so a maker never sells under the outlet
//! nor leaves an ask under it unbought.

use crate::bid_response::BID_MIN_PROFIT;
use crate::market_value::HANDOVER_TICKS;

/// Ticks one unit takes to sell to a keeper: a batch of ten per click,
/// about two ticks and a half (`kit::shop::sell_units`).
pub const SHOP_SELL_TICKS_PER_UNIT: f64 = 0.25;

/// Where the units go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutletKind {
    /// An NPC shop (its key), sold by the janitor's shop trip.
    Shop(String),
    /// High alchemy (the caster's own cast).
    Alchemy,
}

/// One outlet of an object for this bot.
pub struct Outlet<'a> {
    pub kind: OutletKind,
    /// Coins the outlet pays for the unit of rank `k` of the lot (0 for
    /// the first): a shop's price at its likely stock plus `k`, an
    /// alchemy cast's net (runes paid).
    pub unit_value: &'a dyn Fn(i64) -> f64,
    /// Ticks one unit takes there (a sale click share, a cast).
    pub unit_ticks: f64,
    /// Ticks of the trip there and back, once per lot (0 for a cast).
    pub trip_ticks: f64,
}

/// One lot to buy on the market and send to an outlet.
#[derive(Clone, Debug, PartialEq)]
pub struct ArbitrageLot {
    pub item: String,
    pub outlet: OutletKind,
    /// Units bought.
    pub count: i64,
    /// Dearest ask taken: the bid's opening price.
    pub open: i64,
    /// What the outlet pays for the lot's last unit, net of its handling
    /// and its share of the trip at the bot's rate (`net_outlet`), never
    /// under `open`: the bid's ceiling (an unfilled bid climbs 10 % a
    /// pass toward it, the market's walk).
    pub ceiling: i64,
    /// Coins paid for the units.
    pub spent: i64,
    /// Coins the outlet pays for them.
    pub revenue: f64,
    /// Ticks of the lot: the trip once, each unit's handling.
    pub ticks: f64,
    /// revenue - spent - ticks x the bot's rate.
    pub profit: f64,
}

impl ArbitrageLot {
    /// Coins per tick of the lot's ticks, `revenue - spent` over them.
    pub fn rate(&self) -> f64 {
        (self.revenue - self.spent as f64) / self.ticks.max(1.0)
    }
}

/// The lot of `item` worth sending to the best of `outlets` (module
/// doc), `None` when no outlet nets `BID_MIN_PROFIT` after the trip.
/// `asks`: others' resting asks `(price, units)`; `coin_rate`: the bot's
/// time value; `wallet`: its free coins.
pub fn best_lot(item: &str, asks: &[(i64, i64)], outlets: &[Outlet], coin_rate: f64, wallet: i64) -> Option<ArbitrageLot> {
    let mut sorted: Vec<(i64, i64)> = asks.iter().copied().filter(|(price, units)| *price >= 1 && *units > 0).collect();
    sorted.sort_by_key(|(price, _)| *price);
    let mut best: Option<ArbitrageLot> = None;
    for outlet in outlets {
        let Some(lot) = lot_through(item, &sorted, outlet, coin_rate, wallet) else { continue };
        if lot.profit >= BID_MIN_PROFIT && best.as_ref().is_none_or(|known| lot.profit > known.profit) {
            best = Some(lot);
        }
    }
    best
}

/// The lot through one outlet (asks cheapest first): every unit whose
/// outlet value beats its ask and handling, within the wallet (every
/// unit at the dearest ask taken: the bid's commitment); no cap on the
/// count (user 2026-11-19): asks, wallet and outlet bound it. `None`
/// when not even one unit pays.
fn lot_through(item: &str, asks: &[(i64, i64)], outlet: &Outlet, coin_rate: f64, wallet: i64) -> Option<ArbitrageLot> {
    let rate = coin_rate.max(0.0);
    let handling = outlet.unit_ticks.max(0.0) * rate;
    let (mut count, mut spent, mut revenue, mut ceiling) = (0_i64, 0_i64, 0.0_f64, 0_i64);
    'asks: for (price, units) in asks {
        for _ in 0..*units {
            // The bid rests at the dearest ask taken: the wallet covers
            // every unit at that price (the market's own coverage check).
            if (count + 1).saturating_mul(*price) > wallet {
                break 'asks;
            }
            let value = (outlet.unit_value)(count);
            if value <= *price as f64 + handling {
                break 'asks;
            }
            count += 1;
            spent += price;
            revenue += value;
            ceiling = *price;
        }
    }
    if count == 0 {
        return None;
    }
    let ticks = HANDOVER_TICKS + outlet.trip_ticks.max(0.0) + outlet.unit_ticks.max(0.0) * count as f64;
    let profit = revenue - spent as f64 - ticks * rate;
    let last = (outlet.unit_value)(count - 1);
    let net = net_outlet(last, outlet.unit_ticks, outlet.trip_ticks, rate, count).unwrap_or(0);
    Some(ArbitrageLot { item: item.to_string(), outlet: outlet.kind.clone(), count, open: ceiling, ceiling: net.max(ceiling), spent, revenue, ticks, profit })
}

/// What one unit nets through an outlet when `units` of them go there
/// in one trip: the value of the last unit (`last_unit_value`, the
/// shop's price after the others) less its handling and its share of
/// the trip (and the fountain handover) at `coin_rate`, rounded down;
/// `None` under one coin.
pub fn net_outlet(last_unit_value: f64, unit_ticks: f64, trip_ticks: f64, coin_rate: f64, units: i64) -> Option<i64> {
    let rate = coin_rate.max(0.0);
    let share = (HANDOVER_TICKS + trip_ticks.max(0.0)) * rate / units.max(1) as f64;
    let net = (last_unit_value - unit_ticks.max(0.0) * rate - share).floor();
    (net >= 1.0).then_some(net as i64)
}

#[cfg(test)]
#[path = "merchant_arbitrage_tests.rs"]
mod tests;
