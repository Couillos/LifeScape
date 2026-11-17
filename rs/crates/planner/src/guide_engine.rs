//! Shared core of the guides (plan rendements R4): memoised yields of
//! one build, the price rules and the pricing of one yield's flows.
//!
//! Price rules (D12, plan marché §9, user 2026-11-16), always for a
//! VOLUME (the units a row moves per 6000 ticks), never for one unit
//! applied to a window:
//! - SALE of a TRADED object: its current price (median of its last five
//!   fills clamped by the book) for every unit, or an F2P shop's when it
//!   pays more (Q3). A bid lifting the price above the median pays its
//!   own units only. A price resting on fewer than five fills (`thin`)
//!   holds for the units those fills moved only: past them the sale hits
//!   the remaining bids for their depth, then the shop (Q3), else is
//!   worth nothing, as a never-traded object;
//! - SALE of a NEVER-TRADED object (no current price): the resting bids
//!   best first (while they pay more than the shop), the rest to the
//!   best F2P shop (Q3) or worth nothing without one;
//! - PURCHASE: at least the best resting ask: the asks (each unit at
//!   max(current price, ask) for a traded object) and the F2P shops'
//!   window (Q7, decision 2026-11-16 §2: base stock unit by unit at the
//!   server price for the stock left, then the window's restock) walked
//!   together cheapest first for the volume; past both, NOT for sale
//!   (`available` false: the row needing it is blocked, never priced at
//!   a made-up figure);
//! - a shop behind a condition (an F2P quest, quest points; Q7) buys and
//!   sells like any other, its condition carried by the walk
//!   (`Priced::need`) to the row's prerequisites;
//! - a consumable a run must buy (ammo, runes, bait, supply, food, fee)
//!   is a purchase;
//! - a recipe's stock (its `input` flows), lost or discarded objects
//!   cost their SALE price: what the bot gives up by not selling them
//!   (D5, opportunity cost); a 100 % market run buys its inputs (D15).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::guide_flows::{fold_negligible_drops, OtherDrops};
use crate::guide_model::{FlowLine, GuideWorld, Outlet, PriceBasis, Quote, RateAnswer, RateAsk, ShopNeed, WINDOW_TICKS};

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
    /// Conditions of the shops the volume trades with (Q7): the row's
    /// prerequisite (`guide_prereq::require_shop`).
    pub need: ShopNeed,
}

impl Priced {
    fn own() -> Self {
        Priced { price: 1.0, outlet: Outlet::Market, traded: true, basis: PriceBasis::Own, available: true, need: ShopNeed::default() }
    }
}

/// Which side of a book a volume walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    /// Hit the bids, the rest to a shop that buys (Q3), else worth 0.
    Sell,
    /// Lift the asks, the rest from a shop that sells (Q7), else not for
    /// sale.
    Buy,
}

/// What prices one step of a walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// The object's current price (D12, from its fills).
    Current,
    /// Resting orders of the book.
    Book,
    /// An F2P shop (no depth limit).
    Shop,
}

/// One step of a walk: a unit price for at most `depth` units, and what
/// reaching it needs (a shop's condition, Q7; none for the market).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Step {
    price: f64,
    depth: f64,
    tier: Tier,
    need: ShopNeed,
}

/// Resting levels with a price and a depth, best first, as book steps.
fn book_steps(levels: &[(f64, f64)]) -> impl Iterator<Item = Step> + '_ {
    levels.iter().filter(|(price, depth)| *price >= 1.0 && *depth > 0.0).map(|(price, depth)| Step { price: *price, depth: *depth, tier: Tier::Book, need: ShopNeed::default() })
}

/// Walks `steps` in order for `units`: each step takes what it holds,
/// the rest is uncovered (worth nothing for a sale, not for sale for a
/// purchase). `traded` names the basis of an object priced mostly at its
/// current price (`Traded` or `Thin`); the others say what the book and
/// the shop hold of the volume.
fn walk(steps: &[Step], units: f64, side: Side, traded: Option<PriceBasis>) -> Priced {
    let units = if units > 0.0 { units } else { 1.0 };
    let (mut remaining, mut value) = (units, 0.0);
    let (mut at_current, mut at_book, mut at_shop) = (0.0, 0.0, 0.0);
    let mut need = ShopNeed::default();
    for step in steps {
        if remaining <= 0.0 {
            break;
        }
        let taken = step.depth.min(remaining);
        if taken <= 0.0 {
            continue;
        }
        value += taken * step.price;
        remaining -= taken;
        need = need.and(step.need);
        match step.tier {
            Tier::Current => at_current += taken,
            Tier::Book => at_book += taken,
            Tier::Shop => at_shop += taken,
        }
    }
    let uncovered = remaining.max(0.0);
    let available = side == Side::Sell || uncovered <= 1e-9;
    let basis = match (traded, at_book > 0.0, at_shop > 0.0, uncovered > 1e-9) {
        (_, _, _, true) if side == Side::Buy => {
            if at_book + at_current > 0.0 {
                PriceBasis::BookNone
            } else {
                PriceBasis::Nothing
            }
        }
        (Some(basis), _, _, _) if at_current >= at_book + at_shop + uncovered => basis,
        (_, true, false, false) => PriceBasis::Book,
        (_, true, true, _) => PriceBasis::BookShop,
        (_, true, false, true) => PriceBasis::BookNone,
        (_, false, true, _) => PriceBasis::Shop,
        (Some(basis), false, false, false) => basis,
        (_, false, false, _) => PriceBasis::Nothing,
    };
    let outlet = if at_shop > at_book + at_current { Outlet::Shop } else { Outlet::Market };
    let price = if available { value / units } else { 0.0 };
    Priced { price, outlet, traded: basis.weakness() == 0, basis, available, need }
}

/// A traded object's median (its price when the quote carries none).
fn median_of(quote: &Quote) -> f64 {
    if quote.median >= 1.0 {
        quote.median
    } else {
        quote.price
    }
}

/// True when a traded object's current price rests on fewer than
/// `FULL_FILLS` fills (user 2026-11-16: it holds for their units only).
fn thin(quote: &Quote) -> bool {
    quote.fills < crate::guide_model::FULL_FILLS
}

/// Basis of a traded object's own price: thin under `FULL_FILLS`.
fn traded_basis(quote: &Quote) -> PriceBasis {
    if thin(quote) {
        PriceBasis::Thin
    } else {
        PriceBasis::Traded
    }
}

/// Sale steps of a quote, best first (D12, Q3, user 2026-11-16). A
/// traded object: the bids above its price for their units, then its
/// price (for the units its fills moved when thin; every unit else),
/// then, past a thin price, the remaining bids and the shop; a shop
/// paying more than the price takes every unit the bids above it leave.
/// A never-traded object: the bids above the shop, then the shop. A
/// shop step carries the shop's condition (Q7).
fn sale_steps(quote: &Quote, offer: Option<(f64, ShopNeed)>) -> Vec<Step> {
    let shop = offer.map(|(price, _)| price);
    let shop_step = |(price, need): (f64, ShopNeed)| Step { price, depth: f64::INFINITY, tier: Tier::Shop, need };
    let above_shop = |step: &Step| shop.map_or(true, |shop| step.price > shop);
    let mut steps: Vec<Step> = Vec::new();
    let base = quote.price.min(median_of(quote));
    if !quote.traded || shop.is_some_and(|shop| shop > base) {
        steps.extend(book_steps(&quote.bids).take_while(above_shop));
        steps.extend(offer.map(shop_step));
        return steps;
    }
    let bids: Vec<Step> = book_steps(&quote.bids).collect();
    let lifting = bids.iter().take_while(|step| step.price > base).count();
    steps.extend(bids[..lifting].iter().copied());
    let depth = if thin(quote) { quote.fill_units.max(0.0) } else { f64::INFINITY };
    steps.push(Step { price: base, depth, tier: Tier::Current, need: ShopNeed::default() });
    if depth.is_finite() {
        steps.extend(bids[lifting..].iter().copied().take_while(above_shop));
        steps.extend(offer.map(shop_step));
    }
    steps
}

/// Purchase steps of a quote, cheapest first (D15, Q7, user
/// 2026-11-16): the resting asks for their depth, each unit at least the
/// current price of a traded object (max(current price, ask)), and the
/// shops' window steps (finite stock, §2), merged by price (a shop
/// first at a tie), each shop step with its shop's condition (Q7). Past
/// them all: not for sale.
fn purchase_steps(quote: &Quote, shop: &[(f64, f64, ShopNeed)]) -> Vec<Step> {
    let floor = if quote.traded { quote.price.max(1.0) } else { 0.0 };
    let mut steps: Vec<Step> = shop.iter().filter(|(price, units, _)| *price >= 0.0 && *units > 0.0).map(|(price, units, need)| Step { price: *price, depth: *units, tier: Tier::Shop, need: *need }).collect();
    steps.extend(book_steps(&quote.asks).map(|step| if step.price <= floor { Step { price: floor, tier: Tier::Current, ..step } } else { step }));
    // Stable: the shop steps (pushed first) lead at equal prices.
    steps.sort_by(|left, right| left.price.total_cmp(&right.price));
    steps
}

type AskKey = (String, String, Option<(i64, i64, i64)>, String, u8);

/// One build of the guides: the world and the yields asked so far.
pub struct Engine<'a> {
    pub world: &'a GuideWorld<'a>,
    answers: RefCell<HashMap<AskKey, Result<RateAnswer, String>>>,
    quotes: RefCell<HashMap<String, Quote>>,
    sources: RefCell<HashMap<(String, u8, usize, crate::guide_chain::Sourcing), Option<crate::guide_chain::ChainUnit>>>,
    best_profit: RefCell<HashMap<(u8, Vec<String>, bool), Option<f64>>>,
    shops: RefCell<HashMap<String, Vec<(f64, f64, ShopNeed)>>>,
}

impl<'a> Engine<'a> {
    pub fn new(world: &'a GuideWorld<'a>) -> Self {
        Engine {
            world,
            answers: RefCell::new(HashMap::new()),
            quotes: RefCell::new(HashMap::new()),
            sources: RefCell::new(HashMap::new()),
            best_profit: RefCell::new(HashMap::new()),
            shops: RefCell::new(HashMap::new()),
        }
    }

    /// The F2P shops' window of an object (`GuideWorld::shop_buy`), read
    /// once per build.
    pub fn shop_steps(&self, item: &str) -> Vec<(f64, f64, ShopNeed)> {
        if let Some(known) = self.shops.borrow().get(item) {
            return known.clone();
        }
        let found = (self.world.shop_buy)(item);
        self.shops.borrow_mut().insert(item.to_string(), found.clone());
        found
    }

    /// Fastest source of an object at a level and depth, built once per
    /// build (the chains of every recipe share their upstream).
    pub fn source_memo(&self, key: (String, u8, usize, crate::guide_chain::Sourcing), compute: impl FnOnce() -> Option<crate::guide_chain::ChainUnit>) -> Option<crate::guide_chain::ChainUnit> {
        if let Some(known) = self.sources.borrow().get(&key) {
            return known.clone();
        }
        let found = compute();
        self.sources.borrow_mut().insert(key, found.clone());
        found
    }

    /// Best accessible profit of a bot (D17; the flag: chain lines
    /// counted), built once per build.
    pub fn best_profit_memo(&self, key: (u8, Vec<String>, bool), compute: impl FnOnce() -> Option<f64>) -> Option<f64> {
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

    /// Mean sale price of `units` (`sale_steps`). Coins are worth one.
    pub fn sale_at(&self, item: &str, units: f64) -> Priced {
        if item == "coins" {
            return Priced::own();
        }
        let quote = self.quote(item);
        let shop = (self.world.npc_sale)(item);
        walk(&sale_steps(&quote, shop), units, Side::Sell, quote.traded.then(|| traded_basis(&quote)))
    }

    /// Mean purchase price of `units` (`purchase_steps`); `available`
    /// false past the asks' and the shops' depth. Coins are worth one.
    pub fn buy_at(&self, item: &str, units: f64) -> Priced {
        if item == "coins" {
            return Priced::own();
        }
        let quote = self.quote(item);
        walk(&purchase_steps(&quote, &self.shop_steps(item)), units, Side::Buy, quote.traded.then(|| traded_basis(&quote)))
    }

    /// Why `units` of `item` cannot be bought (for a blocked row).
    pub fn unavailable_reason(&self, item: &str, units: f64) -> String {
        let quote = self.quote(item);
        let depth = quote.asks.iter().fold(0.0, |held, (_, depth)| held + depth.max(0.0));
        let traded = if quote.traded { "" } else { "never traded, " };
        let shop: f64 = self.shop_steps(item).iter().map(|(_, units, _)| units.max(0.0)).sum();
        let shops = if shop > 0.0 { format!("F2P shops {shop:.0} (stock + restock)") } else { "no F2P shop sells it".to_string() };
        format!("{item}: {traded}asks hold {depth:.0}, {shops}, of {units:.0} per window")
    }

    /// Mean unit price of one signed flow of `qty` units under `pricing`.
    pub fn flow_price(&self, item: &str, cause: &str, qty: f64, pricing: InputPricing) -> Priced {
        if qty >= 0.0 {
            return self.sale_at(item, qty);
        }
        if bought(cause, pricing) {
            self.buy_at(item, -qty)
        } else {
            self.sale_at(item, -qty)
        }
    }
}

/// True when an outgoing flow of `cause` is bought (a consumable, or a
/// 100 % market input), not valued at its sale price.
fn bought(cause: &str, pricing: InputPricing) -> bool {
    BOUGHT_CAUSES.contains(&cause) || (cause == "input" && pricing == InputPricing::Bought)
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
    /// Conditions of the shops the row sells its output to or buys from
    /// (Q7): its prerequisite. A stock input valued at its sale price
    /// (D5) trades with nobody: none.
    pub need: ShopNeed,
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
        if per_6000 >= 0.0 || bought(&cause, pricing) {
            out.need = out.need.and(priced.need);
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
