//! Shared core of the guides (plan rendements R4): memoised yields of
//! one build, the price rules and the pricing of one yield's flows.
//!
//! Price rules (D12, plan marché §9):
//! - a product sells at the current market price, or to an F2P shop
//!   when the shop pays more (Q3, outlet `shop`);
//! - a consumable a run must buy (ammo, runes, bait, supply, food, fee)
//!   costs the current price, or the shop's when a shop sells it
//!   cheaper (Q7);
//! - a recipe's stock (its `input` flows), lost or discarded objects
//!   cost their SALE price: what the bot gives up by not selling them
//!   (D5, opportunity cost); a 100 % market run buys its inputs (D15).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::guide_flows::{fold_negligible_drops, OtherDrops};
use crate::guide_model::{FlowLine, GuideWorld, Outlet, RateAnswer, RateAsk, WINDOW_TICKS};

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

/// A price and where it comes from; `available` is false for a
/// purchase nobody offers (no fill, no ask, no shop).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priced {
    pub price: f64,
    pub outlet: Outlet,
    pub traded: bool,
    pub available: bool,
}

type AskKey = (String, String, Option<(i64, i64, i64)>, String, u8);

/// One build of the guides: the world and the yields asked so far.
pub struct Engine<'a> {
    pub world: &'a GuideWorld<'a>,
    answers: RefCell<HashMap<AskKey, Result<RateAnswer, String>>>,
    sales: RefCell<HashMap<String, Priced>>,
    buys: RefCell<HashMap<String, Priced>>,
    sources: RefCell<HashMap<(String, u8, usize), Option<crate::guide_chain::ChainUnit>>>,
    best_profit: RefCell<HashMap<(u8, Vec<String>), Option<f64>>>,
}

impl<'a> Engine<'a> {
    pub fn new(world: &'a GuideWorld<'a>) -> Self {
        Engine {
            world,
            answers: RefCell::new(HashMap::new()),
            sales: RefCell::new(HashMap::new()),
            buys: RefCell::new(HashMap::new()),
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

    /// Sale price of one unit: the market's current price, or the shop's
    /// when it pays more (Q3). Coins are worth one.
    pub fn sale(&self, item: &str) -> Priced {
        if item == "coins" {
            return Priced { price: 1.0, outlet: Outlet::Market, traded: true, available: true };
        }
        if let Some(known) = self.sales.borrow().get(item) {
            return *known;
        }
        let quote = (self.world.price)(item);
        let found = match (self.world.npc_sale)(item) {
            Some(shop) if shop > quote.price => Priced { price: shop, outlet: Outlet::Shop, traded: true, available: true },
            _ => Priced { price: quote.price, outlet: Outlet::Market, traded: quote.traded, available: true },
        };
        self.sales.borrow_mut().insert(item.to_string(), found);
        found
    }

    /// Purchase price of one unit: the market's current price, or the
    /// shop's when it sells cheaper or the market offers none (Q7);
    /// unavailable when neither sells it. Coins are worth one.
    pub fn buy(&self, item: &str) -> Priced {
        if item == "coins" {
            return Priced { price: 1.0, outlet: Outlet::Market, traded: true, available: true };
        }
        if let Some(known) = self.buys.borrow().get(item) {
            return *known;
        }
        let quote = (self.world.price)(item);
        let found = match (self.world.shop_buy)(item) {
            Some(shop) if shop < quote.price || !quote.buyable => Priced { price: shop, outlet: Outlet::Shop, traded: true, available: true },
            _ => Priced { price: quote.price, outlet: Outlet::Market, traded: quote.traded, available: quote.buyable },
        };
        self.buys.borrow_mut().insert(item.to_string(), found);
        found
    }

    /// Unit price of one signed flow under `pricing`.
    pub fn flow_price(&self, item: &str, cause: &str, qty: f64, pricing: InputPricing) -> Priced {
        if qty >= 0.0 {
            return self.sale(item);
        }
        if BOUGHT_CAUSES.contains(&cause) || (cause == "input" && pricing == InputPricing::Bought) {
            self.buy(item)
        } else {
            self.sale(item)
        }
    }
}

/// The priced lines of a yield's flows scaled by `scale` (work over the
/// row's total time), rare pickups folded (`guide_flows`), `skip`ped
/// items left out (farmed upstream, priced by the chain); returns the
/// lines, the folded drops, revenue and cost per 6000 ticks of the row.
pub fn price_flows(engine: &Engine, answer: &RateAnswer, own: Option<&str>, pricing: InputPricing, scale: f64, skip: &dyn Fn(&str, &str) -> bool) -> (Vec<FlowLine>, OtherDrops, f64, f64) {
    let kept_flows: Vec<(String, String, f64)> = answer.flows.iter().filter(|(item, cause, _)| !skip(item, cause)).cloned().collect();
    let (kept, mut other) = fold_negligible_drops(&kept_flows, &|item| own == Some(item), &|item| engine.sale(item).price);
    other.units_per_6000 *= scale;
    other.value_per_6000 *= scale;
    let mut lines = Vec::new();
    let mut revenue = other.value_per_6000.max(0.0);
    let mut cost = (-other.value_per_6000).max(0.0);
    for (item, cause, qty) in kept {
        let priced = engine.flow_price(&item, &cause, qty, pricing);
        let per_6000 = qty * scale;
        let value = per_6000 * priced.price;
        if value >= 0.0 {
            revenue += value;
        } else {
            cost -= value;
        }
        lines.push(FlowLine { item, cause, per_6000, unit_price: priced.price, value_6000: value, outlet: priced.outlet, traded: priced.traded });
    }
    (lines, other, revenue, cost)
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
