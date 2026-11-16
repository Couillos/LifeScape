#![forbid(unsafe_code)]
//! Dedicated market makers (plan marché Q6, wave 3): accounts that quote
//! BOTH sides of a fixed slice of the F2P universe, for profit, from
//! public information only (D22) plus their own stock and capital.
//!
//! Quote (one item, one pass), `quote`:
//! - fair value `v`: the current traded price (`Reference`); else the
//!   middle of the others' best bid and ask; else, for a never-traded
//!   object (prudent discovery), one spread under the public ceiling a
//!   buyer faces (its shop value, or the cheapest listing when lower):
//!   a quarter lot, never raised by a low stock;
//! - stock skew (inventory risk): `r = clamp((stock - lot) / lot, -1, 1)`,
//!   reservation price `p = v * (1 - h * r)`: a high stock lowers BOTH
//!   sides (sell faster, buy less), a low stock raises both;
//! - legs: `bid = floor(p * (1 - h))`, never over `v - 1` (always under
//!   the current price); `ask = ceil(p * (1 + h))`, never under `v + 1`
//!   (always over it), over the bid and over what a shop pays (Q3);
//! - half spread `h` by rotation: liquid 4 %, thin 8 %, discovery 10 %;
//! - no demand (traded before, no other bid, no buyer over the last
//!   `DEMAND_LOOKBACK_TICKS`, a loot horizon): the bid walks down 5 %
//!   per pass toward 1 and the stock past one lot goes to the NPC shop
//!   (Q3, `npc_dump`);
//! - risk caps (`MakerCaps`): capital engaged per object (stock at its
//!   value or its cost, the higher, plus the bid) ≤ 10 % of the
//!   capital, stock ≤ 500 units,
//!   the maker's units on a side ≤ 25 % of that side (others' depth
//!   counts; alone on a side it posts one lot: it IS the liquidity).
//!
//! Slices (`partition`): the universe is split over N accounts, N =
//! universe / slice capacity (F2P bank 60 types + pack 28, minus the
//! slots a maker keeps, times a fill margin), each object assigned by
//! highest random weight with a load cap: deterministic, and stable when
//! an object joins the universe while the load cap holds (only that
//! object moves).

use crate::price::Reference;

/// Bank types of an F2P account (`bank.constant ^bank_free_slots = 60`,
/// content `interface_bank`).
pub const BANK_SLOTS: usize = 60;
/// Inventory slots.
pub const PACK_SLOTS: usize = 28;
/// Slots a maker never fills with slice stock: coins (1), the standing
/// kit kept on every bot (axe, pickaxe, hammer, tinderbox: 4), food (1),
/// and half the pack (14) free for deliveries (notes withdrawn for the
/// grouped deals of one convocation).
pub const RESERVED_SLOTS: usize = 1 + 4 + 1 + 14;
/// Share of the free slots a slice fills: room for stray objects (loot
/// kept from the playing phase) and a universe that grows.
pub const FILL_MARGIN_PCT: usize = 80;

/// How far back a maker looks for buyers before calling an object
/// without demand: one loot horizon (`market_lineage` 72 000 ticks), a
/// looter's whole concession.
pub const DEMAND_LOOKBACK_TICKS: u64 = 72_000;

/// No demand: the resting bid walks down 5 % per pass (at least a coin)
/// toward 1, the old ZIP step: from 100 coins to 1 in ~45 passes, inside
/// one loot horizon at a pass every 1000 ticks.
pub const NO_DEMAND_STEP: f64 = 0.05;

/// Objects one maker covers: (60 + 28 - 20) * 80 % = 54.
pub fn slice_capacity() -> usize {
    (BANK_SLOTS + PACK_SLOTS - RESERVED_SLOTS) * FILL_MARGIN_PCT / 100
}

/// Makers needed to cover `universe` objects.
pub fn maker_count(universe: usize) -> usize {
    universe.div_ceil(slice_capacity().max(1))
}

fn weight(item: i64, slice: usize) -> u64 {
    let mut word = (item as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (slice as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    word ^ (word >> 31)
}

/// Splits `universe` (object ids) over `slices` accounts: every object
/// in exactly one slice, each slice at most `ceil(universe / slices)`
/// objects. Objects are taken in id order and each goes to its highest
/// weight slice with room (rendezvous hashing with a load cap): the same
/// universe always gives the same slices, and an object added past the
/// others moves nothing else. Each slice is sorted by id.
pub fn partition(universe: &[i64], slices: usize) -> Vec<Vec<i64>> {
    let slices = slices.max(1);
    let mut items: Vec<i64> = universe.to_vec();
    items.sort_unstable();
    items.dedup();
    let cap = items.len().div_ceil(slices).max(1);
    let mut out: Vec<Vec<i64>> = vec![Vec::new(); slices];
    for item in items {
        let mut order: Vec<usize> = (0..slices).collect();
        order.sort_by_key(|slice| std::cmp::Reverse(weight(item, *slice)));
        if let Some(slice) = order.into_iter().find(|slice| out[*slice].len() < cap) {
            out[slice].push(item);
        }
    }
    out
}

/// Risk caps and quote shape of a maker (named, plan Q6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MakerCaps {
    /// Value of one quote lot in coins (lot units = this / fair value).
    pub lot_value: i64,
    /// Max share of the capital engaged in one object (stock + bid).
    pub max_item_capital_share: f64,
    /// Max units of one object held or bid.
    pub max_stock: i64,
    /// Max share of one book side held by the maker.
    pub max_book_share: f64,
    /// Half spreads: traded with a lot's volume lately, traded thin or
    /// priced off the book, never traded.
    pub half_spread_liquid: f64,
    pub half_spread_thin: f64,
    pub half_spread_discovery: f64,
}

/// The caps every maker runs with: one lot is worth 1000 coins (a lot of
/// feathers is 500 units, of steel bars ~12, of a rune platebody 1),
/// 10 % of the capital per object, 500 units, 25 % of a side; 4 / 8 /
/// 10 % half spreads (a liquid round trip earns ~8 %, a ×1.09 spread; a
/// discovery quote ×1.22: all under the plan's ×1.25 target).
pub const MAKER_CAPS: MakerCaps = MakerCaps {
    lot_value: 1000,
    max_item_capital_share: 0.10,
    max_stock: 500,
    max_book_share: 0.25,
    half_spread_liquid: 0.04,
    half_spread_thin: 0.08,
    half_spread_discovery: 0.10,
};

/// Units of one quote lot at fair value `fair`.
pub fn lot_units(caps: &MakerCaps, fair: i64) -> i64 {
    (caps.lot_value / fair.max(1)).clamp(1, caps.max_stock)
}

/// Capital target of a slice (the switch from the playing phase to the
/// maker role): what posting one bid lot on every object of the slice at
/// once costs at its public anchor, each object capped at its risk share
/// of that very capital (fixed point of `T = sum(min(lot_i, share * T))`,
/// rounded up to 1000 coins). `anchors`: public value per object.
pub fn capital_target(caps: &MakerCaps, anchors: &[i64]) -> i64 {
    let lots: Vec<f64> = anchors.iter().map(|anchor| (lot_units(caps, *anchor) * (*anchor).max(1)) as f64).collect();
    let mut target: f64 = lots.iter().sum();
    for _ in 0..200 {
        let next: f64 = lots.iter().map(|lot| lot.min(caps.max_item_capital_share * target)).sum();
        if (next - target).abs() < 0.5 {
            target = next;
            break;
        }
        target = next;
    }
    ((target.ceil() as i64 + 999) / 1000).max(1) * 1000
}

/// Everything one quote reads: public data (D22) and the maker's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MakerInput {
    /// Current price (median of the last fills, `price::current_price`).
    pub reference: Reference,
    /// Others' best bid and ask (the maker's own orders left out).
    pub best_bid: Option<i64>,
    pub best_ask: Option<i64>,
    /// Others' resting units on each side.
    pub bid_depth: i64,
    pub ask_depth: i64,
    /// Units bought by others lately (public prints, the maker's own
    /// purchases left out): the demand.
    pub recent_buys: i64,
    /// Public anchor of a never-traded object: its shop value.
    pub anchor: i64,
    /// What an F2P shop pays for one unit (Q3 outside option).
    pub outside: Option<i64>,
    /// The maker's own stock (bank, pack) and equity (coins plus stock
    /// at the current price).
    pub stock: i64,
    /// Coins the maker paid for the stock it holds (its own buys).
    pub stock_cost: i64,
    pub capital: i64,
    /// The maker's resting bid, if any (the no-demand walk toward 1).
    pub own_bid: Option<i64>,
}

/// How the fair value was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    /// Traded with at least a lot of buyers' volume lately.
    Liquid,
    /// Traded, little volume; or priced off the others' book.
    Thin,
    /// Never traded: public anchor, prudent.
    Discovery,
    /// Traded before, nobody buys now: bid toward 1, surplus to the shop.
    NoDemand,
}

/// One leg: price and units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Leg {
    pub price: i64,
    pub qty: i64,
}

/// One pass of quotes for one object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MakerQuote {
    pub bid: Option<Leg>,
    pub ask: Option<Leg>,
    /// Units to sell to the NPC shop (Q3): stock past one lot of an
    /// object nobody buys, or past the stock cap.
    pub npc_dump: i64,
    pub fair: i64,
    pub regime: Regime,
}

fn book_cap(caps: &MakerCaps, others: i64, lot: i64) -> i64 {
    if others <= 0 {
        return lot;
    }
    ((others as f64 * caps.max_book_share / (1.0 - caps.max_book_share)).floor() as i64).max(1)
}

/// The maker's quotes for one object (see the module rules).
pub fn quote(caps: &MakerCaps, input: &MakerInput) -> MakerQuote {
    let demand = input.recent_buys > 0 || input.best_bid.is_some();
    let (fair, regime) = if input.reference.traded {
        let lot = lot_units(caps, input.reference.price);
        let regime = if !demand {
            Regime::NoDemand
        } else if input.recent_buys >= lot {
            Regime::Liquid
        } else {
            Regime::Thin
        };
        (input.reference.price.max(1), regime)
    } else if let (Some(bid), Some(ask)) = (input.best_bid, input.best_ask) {
        (((bid + ask) / 2).max(1), Regime::Thin)
    } else {
        // Never traded: a buyer pays at most the shop value or the
        // cheapest public listing; the maker's ask joins that ceiling and
        // its bid sits one spread under (fair = ceiling / (1 + h)).
        let floor = input.outside.map_or(1, |paid| paid + 1);
        let ceiling = input.best_ask.map_or(input.anchor, |ask| ask.min(input.anchor)).max(floor).max(1);
        (((ceiling as f64 / (1.0 + caps.half_spread_discovery)).floor() as i64).max(1), Regime::Discovery)
    };
    let half = match regime {
        Regime::Liquid => caps.half_spread_liquid,
        Regime::Thin | Regime::NoDemand => caps.half_spread_thin,
        Regime::Discovery => caps.half_spread_discovery,
    };
    let lot = lot_units(caps, fair);
    // Discovery never raises its quotes on a low stock (prudence).
    let low = if regime == Regime::Discovery { 0.0 } else { -1.0 };
    let skew = ((input.stock - lot) as f64 / lot as f64).clamp(low, 1.0);
    let reservation = fair as f64 * (1.0 - half * skew);
    let mut bid_price = (reservation * (1.0 - half) + 1e-9).floor() as i64;
    if fair >= 2 {
        bid_price = bid_price.min(fair - 1);
    }
    bid_price = bid_price.max(1);
    // Discovery buys prudently: a quarter lot.
    let bid_lot = if regime == Regime::Discovery { (lot / 4).max(1) } else { lot };
    if regime == Regime::NoDemand {
        let walked = input.own_bid.map_or(bid_price, |own| (own as f64 * (1.0 - NO_DEMAND_STEP)).floor() as i64);
        bid_price = bid_price.min(walked).max(1);
    }
    let mut ask_price = (reservation * (1.0 + half) - 1e-9).ceil() as i64;
    ask_price = ask_price.max(fair + 1).max(bid_price + 1);
    if let Some(paid) = input.outside {
        ask_price = ask_price.max(paid + 1);
    }
    // Risk caps: capital engaged per object, stock, book share.
    // Engaged = the stock at its current value or at what it cost, the
    // higher: a falling price never reopens room (no averaging down).
    let engaged = input.stock.max(0).saturating_mul(fair).max(input.stock_cost.max(0));
    let room_coins = (caps.max_item_capital_share * input.capital.max(0) as f64) as i64 - engaged;
    let by_capital = if room_coins > 0 { room_coins / bid_price } else { 0 };
    let by_stock = caps.max_stock - input.stock.max(0);
    let bid_qty = bid_lot.min(by_capital).min(by_stock).min(book_cap(caps, input.bid_depth, bid_lot));
    let bid = (bid_qty > 0).then_some(Leg { price: bid_price, qty: bid_qty });
    let ask_qty = input.stock.max(0).min(lot).min(book_cap(caps, input.ask_depth, lot));
    let ask = (ask_qty > 0).then_some(Leg { price: ask_price, qty: ask_qty });
    let over_cap = (input.stock - caps.max_stock).max(0);
    let unsold = if regime == Regime::NoDemand { (input.stock - lot).max(0) } else { 0 };
    let npc_dump = if input.outside.is_some_and(|paid| paid >= 1) { over_cap.max(unsold) } else { 0 };
    MakerQuote { bid, ask, npc_dump, fair, regime }
}

#[cfg(test)]
#[path = "maker_tests.rs"]
mod tests;
