//! Shared core of the guides (plan rendements R4): memoised yields of
//! one build, the price rules and the pricing of one yield's flows.
//!
//! Price rules (D12, plan marché §9), always for a VOLUME (the units a
//! row moves per 6000 ticks), never for one unit applied to a window:
//! - a TRADED object has a current price (median of its last five fills
//!   clamped by the book): every unit at it, or at an F2P shop when the
//!   shop pays more for a sale (Q3) or asks less for a purchase (Q7). A
//!   clamp moves the price of the clamping orders' units only: a bid
//!   lifting it above the median pays its own units, the rest sells at
//!   the median; an ask cutting it under the median sells its own units,
//!   the rest is bought at the median. A median of fewer than five fills
//!   is flagged `thin`;
//! - a NEVER-TRADED object has no current price: its book counts only
//!   for the units it holds. A sale fills the resting bids best first
//!   (as long as they pay more than the shop), the rest goes to the best
//!   F2P shop (Q3) or is worth nothing without one; a purchase fills the
//!   resting asks best first (while cheaper than the shop), the rest
//!   comes from an F2P shop (Q7) or is NOT for sale (`available` false:
//!   the row needing it is blocked, never priced at a made-up figure);
//! - a consumable a run must buy (ammo, runes, bait, supply, food, fee)
//!   is a purchase;
//! - a recipe's stock (its `input` flows), lost or discarded objects
//!   cost their SALE price: what the bot gives up by not selling them
//!   (D5, opportunity cost); a 100 % market run buys its inputs (D15).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::guide_flows::{fold_negligible_drops, OtherDrops};
use crate::guide_model::{FlowLine, GuideWorld, Outlet, PriceBasis, Quote, RateAnswer, RateAsk, WINDOW_TICKS};

/// Causes of the consumables a run buys.
pub const BOUGHT_CAUSES: [&str; 6] = ["ammo", "rune", "bait", "supply", "food", "fee"];

/// How a row prices its `input` flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPricing {
    /// At their sale price (owned stock, D5).
    Stock,
    /// At their purchase price (100 % market, D15).
    Bought,
}

/// The mean unit price of a volume and where it comes from; `available`
/// is false for a purchase nobody covers (no fill, asks short of the
/// volume, no shop): its price is then 0 and means nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priced {
    pub price: f64,
    pub outlet: Outlet,
    pub traded: bool,
    pub basis: PriceBasis,
    pub available: bool,
}

impl Priced {
    fn own() -> Self {
        Priced { price: 1.0, outlet: Outlet::Market, traded: true, basis: PriceBasis::Own, available: true }
    }
}

/// Which side of a never-traded object's book a volume walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Hit the bids, the rest to a shop that buys (Q3).
    Sell,
    /// Lift the asks, the rest from a shop that sells (Q7).
    Buy,
}

/// Walks one side of a book for `units`: the resting levels best first
/// while they beat `rest`, the rest of the units at `rest` (a traded
/// object's current price, or a shop), else uncovered. `traded`: the
/// rest is the object's own price (its basis names the line); else the
/// line says how much of it the book of a never-traded object holds.
fn walk_book(levels: &[(f64, f64)], units: f64, rest: Option<(f64, PriceBasis)>, side: Side, traded: bool) -> Priced {
    let units = if units > 0.0 { units } else { 1.0 };
    let beats_rest = |price: f64| match (rest, side) {
        (Some((rest, _)), Side::Sell) => price > rest,
        (Some((rest, _)), Side::Buy) => price < rest,
        (None, _) => true,
    };
    let mut remaining = units;
    let mut value = 0.0;
    let mut at_book = 0.0;
    for (price, depth) in levels.iter().filter(|(price, depth)| *price >= 1.0 && *depth > 0.0) {
        if remaining <= 0.0 || !beats_rest(*price) {
            break;
        }
        let taken = depth.min(remaining);
        value += taken * price;
        at_book += taken;
        remaining -= taken;
    }
    let at_rest = if remaining > 0.0 { rest.map(|(price, basis)| (remaining, price, basis)) } else { None };
    if let Some((taken, price, _)) = at_rest {
        value += taken * price;
        remaining = 0.0;
    }
    let basis = match (traded, rest, at_book > 0.0, at_rest.is_some(), remaining > 0.0) {
        (true, Some((_, basis)), _, _, _) => basis,
        (_, _, true, false, false) => PriceBasis::Book,
        (_, _, true, true, _) => PriceBasis::BookShop,
        (_, _, true, false, true) => PriceBasis::BookNone,
        (_, _, false, true, _) => PriceBasis::Shop,
        (_, _, false, false, _) => PriceBasis::Nothing,
    };
    let at_shop = at_rest.filter(|(_, _, basis)| *basis == PriceBasis::Shop).map(|(taken, _, _)| taken).unwrap_or(0.0);
    let outlet = if at_shop > at_book { Outlet::Shop } else { Outlet::Market };
    let available = side == Side::Sell || remaining <= 0.0;
    let price = if available { value / units } else { 0.0 };
    Priced { price, outlet, traded: basis.weakness() == 0, basis, available }
}

/// A traded object's median (its price when the quote carries none).
fn median_of(quote: &Quote) -> f64 {
    if quote.median >= 1.0 {
        quote.median
    } else {
        quote.price
    }
}

/// Basis of a traded object's own price: thin under `FULL_FILLS`.
fn traded_basis(quote: &Quote) -> PriceBasis {
    if quote.fills < crate::guide_model::FULL_FILLS {
        PriceBasis::Thin
    } else {
        PriceBasis::Traded
    }
}

type AskKey = (String, String, Option<(i64, i64, i64)>, String, u8);

/// One build of the guides: the world and the yields asked so far.
pub struct Engine<'a> {
    pub world: &'a GuideWorld<'a>,
    answers: RefCell<HashMap<AskKey, Result<RateAnswer, String>>>,
    quotes: RefCell<HashMap<String, Quote>>,
    sources: RefCell<HashMap<(String, u8, usize), Option<crate::guide_chain::ChainUnit>>>,
    best_profit: RefCell<HashMap<(u8, Vec<String>), Option<f64>>>,
}

impl<'a> Engine<'a> {
    pub fn new(world: &'a GuideWorld<'a>) -> Self {
        Engine {
            world,
            answers: RefCell::new(HashMap::new()),
            quotes: RefCell::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            best_profit: RefCell::new(HashMap::new()),
        }
    }

    /// Fastest source of an object at a level and depth, built once per
    /// build (the chains of every recipe share their upstream).
    pub fn source_memo(&self, key: (String, u8, usize), compute: impl FnOnce() -> Option<crate::guide_chain::ChainUnit>) -> Option<crate::guide_chain::ChainUnit> {
        if let Some(known) = self.sources.borrow().get(&key) {
            return known.clone();
        }
        let found = compute();
        self.sources.borrow_mut().insert(key, found.clone());
        found
    }

    /// Best accessible profit of a bot (D17), built once per build.
    pub fn best_profit_memo(&self, key: (u8, Vec<String>), compute: impl FnOnce() -> Option<f64>) -> Option<f64> {
        if let Some(known) = self.best_profit.borrow().get(&key) {
            return *known;
        }
        let found = compute();
        self.best_profit.borrow_mut().insert(key, found);
        found
    }

    /// The yield of one ask, asked once per build.
    pub fn answer(&self, ask: &RateAsk) -> Result<RateAnswer, String> {
        let key = (ask.method.clone(), ask.style.clone(), ask.site, ask.tool.clone(), ask.level);
        if let Some(known) = self.answers.borrow().get(&key) {
            return known.clone();
        }
        let found = (self.world.rate)(ask);
        self.answers.borrow_mut().insert(key, found.clone());
        found
    }

    /// The market quote of an object, read once per build.
    pub fn quote(&self, item: &str) -> Quote {
        if let Some(known) = self.quotes.borrow().get(item) {
            return known.clone();
        }
        let found = (self.world.price)(item);
        self.quotes.borrow_mut().insert(item.to_string(), found.clone());
        found
    }

    /// Mean sale price of `units`. A traded object: its current price,
    /// a bid's lift above the median for the bid's units only, or the
    /// shop's when it pays more (Q3). A never-traded object: its bids for
    /// the units they hold, the rest to the shop or worth nothing. Coins
    /// are worth one.
    pub fn sale_at(&self, item: &str, units: f64) -> Priced {
        if item == "coins" {
            return Priced::own();
        }
        let quote = self.quote(item);
        let shop = (self.world.npc_sale)(item);
        if quote.traded {
            let base = quote.price.min(median_of(&quote));
            let rest = match shop {
                Some(shop) if shop > base => (shop, PriceBasis::Shop),
                _ => (base, traded_basis(&quote)),
            };
            return walk_book(&quote.bids, units, Some(rest), Side::Sell, true);
        }
        walk_book(&quote.bids, units, shop.map(|price| (price, PriceBasis::Shop)), Side::Sell, false)
    }

    /// Mean purchase price of `units`. A traded object: its current
    /// price, an ask's cut under the median for the ask's units only, or
    /// the shop's when it sells cheaper (Q7). A never-traded object: its
    /// asks for the units they hold, the rest from a shop, else not for
    /// sale (`available` false). Coins are worth one.
    pub fn buy_at(&self, item: &str, units: f64) -> Priced {
        if item == "coins" {
            return Priced::own();
        }
        let quote = self.quote(item);
        let shop = (self.world.shop_buy)(item);
        if quote.traded {
            let base = quote.price.max(median_of(&quote));
            let rest = match shop {
                Some(shop) if shop < base => (shop, PriceBasis::Shop),
                _ => (base, traded_basis(&quote)),
            };
            return walk_book(&quote.asks, units, Some(rest), Side::Buy, true);
        }
        walk_book(&quote.asks, units, shop.map(|price| (price, PriceBasis::Shop)), Side::Buy, false)
    }

    /// Why `units` of `item` cannot be bought (for a blocked row).
    pub fn unavailable_reason(&self, item: &str, units: f64) -> String {
        let depth = self.quote(item).asks.iter().fold(0.0, |held, (_, depth)| held + depth.max(0.0));
        format!("{item}: never traded, asks hold {depth:.0} of {units:.0} per window, no F2P shop sells it")
    }

    /// Mean unit price of one signed flow of `qty` units under `pricing`.
    pub fn flow_price(&self, item: &str, cause: &str, qty: f64, pricing: InputPricing) -> Priced {
        if qty >= 0.0 {
            return self.sale_at(item, qty);
        }
        if BOUGHT_CAUSES.contains(&cause) || (cause == "input" && pricing == InputPricing::Bought) {
            self.buy_at(item, -qty)
        } else {
            self.sale_at(item, -qty)
        }
    }
}

/// Priced flows of one yield over a row's time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PricedFlows {
    pub lines: Vec<FlowLine>,
    pub other: OtherDrops,
    pub revenue: f64,
    pub cost: f64,
    /// Reasons of the purchases nobody covers (the row is blocked).
    pub missing: Vec<String>,
}

/// The priced lines of a yield's flows scaled by `scale` (the share of
/// the row's time its work fills, times the share of the work its stock
/// covers), rare pickups folded (`guide_flows`), `skip`ped items left
/// out (farmed upstream, priced by the chain); every flow priced at the
/// volume it moves per 6000 ticks of the row.
pub fn price_flows(engine: &Engine, answer: &RateAnswer, own: Option<&str>, pricing: InputPricing, scale: f64, skip: &dyn Fn(&str, &str) -> bool) -> PricedFlows {
    let kept_flows: Vec<(String, String, f64)> = answer.flows.iter().filter(|(item, cause, _)| !skip(item, cause)).cloned().collect();
    let (kept, mut other) = fold_negligible_drops(&kept_flows, &|item| own == Some(item), &|item| engine.sale_at(item, 1.0).price);
    other.units_per_6000 *= scale;
    other.value_per_6000 *= scale;
    let mut out = PricedFlows { revenue: other.value_per_6000.max(0.0), cost: (-other.value_per_6000).max(0.0), other, ..PricedFlows::default() };
    for (item, cause, qty) in kept {
        let per_6000 = qty * scale;
        let priced = engine.flow_price(&item, &cause, per_6000, pricing);
        if !priced.available {
            out.missing.push(engine.unavailable_reason(&item, -per_6000));
        }
        let value = per_6000 * priced.price;
        if value >= 0.0 {
            out.revenue += value;
        } else {
            out.cost -= value;
        }
        out.lines.push(FlowLine { item, cause, per_6000, unit_price: priced.price, value_6000: value, outlet: priced.outlet, traded: priced.traded, basis: priced.basis });
    }
    out
}

/// Work share of a row whose 6000 work ticks carry `extra` more ticks.
pub fn work_share(extra_ticks: f64) -> f64 {
    WINDOW_TICKS / (WINDOW_TICKS + extra_ticks.max(0.0))
}

/// Chebyshev walk in ticks from `from` to `to` and back (one tile a
/// tick, the rates prior's walk model); `None` across planes.
pub fn round_trip_ticks(from: (i64, i64, i64), to: (i64, i64, i64)) -> Option<f64> {
    (from.2 == to.2).then(|| 2.0 * (from.0 - to.0).abs().max((from.1 - to.1).abs()) as f64 / crate::method_rate::WALK_TILES_PER_TICK)
}

/// True for a stackable object (fills no slot per unit).
pub fn stackable(item: &str) -> bool {
    lifescape_gamedata::objs_by_name(item).first().is_some_and(|obj| obj.stackable)
}

/// Lots (packs of `LOT_SLOTS` non-stackable inputs) one window of work
/// moves; 0 when every input stacks (one lot is then the window).
pub fn lots_per_6000(answer: &RateAnswer) -> f64 {
    let slots: f64 = answer.flows.iter().filter(|(item, cause, qty)| cause == "input" && *qty < 0.0 && !stackable(item)).map(|(_, _, qty)| -qty).sum();
    slots / crate::guide_model::LOT_SLOTS
}

/// Stock inputs of one window of work: (item, units), summed per item.
pub fn stock_inputs(answer: &RateAnswer) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for (item, _, qty) in answer.flows.iter().filter(|(_, cause, qty)| cause == "input" && *qty < 0.0) {
        match out.iter_mut().find(|(known, _)| known == item) {
            Some(known) => known.1 -= qty,
            None => out.push((item.clone(), -qty)),
        }
    }
    out
}

/// One lot of a recipe's stock (D5 prerequisite): each input's units of
/// a pack's worth of work, whole units.
pub fn stock_lot(answer: &RateAnswer) -> Vec<(String, f64)> {
    let lots = lots_per_6000(answer);
    stock_inputs(answer).into_iter().map(|(item, units)| (item, if lots > 0.0 { (units / lots).ceil() } else { units.ceil() })).collect()
}
