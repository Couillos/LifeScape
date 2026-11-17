//! Chains of the guides (plan rendements D16): farm the inputs, then
//! transform them (ore -> bar -> object), every stage compared to
//! selling at that stage.
//!
//! One unit of a stage's product costs the stage's own work (6000 ticks
//! per its measured or prior rate of that product, pauses included) plus,
//! for every input, the input's quantity times the ticks of one unit of
//! the input's fastest SOURCE (a farm activity or another chain, at most
//! `MAX_CHAIN_DEPTH` deep, never through an object already in the path).
//! A source is a production leg of the object, or an activity of the
//! catalogue whose guide prior yields it as a by-product or a drop (an
//! uncut gem: one mining roll in 256, dump `mining_gems` of
//! `mining.rs2`; a fight's drop table): one unit then costs 6000 ticks
//! over that activity's rate of it, however rare.
//!
//! What a chain sources (decisions 2026-11-16 §1 and §3, `sourceable`):
//! every recipe input and every consumable of the method (runes, ammo,
//! bait, supply, food; never a fee nor coins). How:
//! - `Sourcing::Cheapest` (money chains, D16 revised): each one the
//!   CHEAPER way, farming (its ticks at the bot's D17 price, less the
//!   other flows of its source) or buying (D15 prices at its volume:
//!   asks, finite shop stock); a tie farms; neither: missing;
//! - `Sourcing::Farm` (autonomous training, D13: the bot obtains
//!   everything itself): farmed whenever an activity makes it at the
//!   level; an input nothing makes is missing (the chain cannot run), a
//!   consumable nothing makes is bought (runes of nature or law...).
//!
//! Every choice is kept (`InputRoute`) for the guide to show. The value
//! of a unit counts every other flow of every stage over its time share
//! (a fight's other loot, a smelter's coal...): only the farmed
//! intermediates cancel. Selling at a stage pays its product's sale price
//! (D12/Q3) over the ticks of that stage and its upstream.

use crate::guide_engine::{Engine, InputPricing};
use crate::guide_model::{InputRoute, Prereqs, RateAsk, Route, Stage, MAX_CHAIN_DEPTH};

/// Style a source fight is read in (its drops do not depend on it).
pub const SOURCE_FIGHT_STYLE: &str = "melee:attack";

/// The bot's price of time (D17: coins per 6000 ticks of its best
/// accessible money line), hashable for the memo of sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimePrice(u64);

impl TimePrice {
    /// `coins_6000` per 6000 ticks (a negative or missing profit: time
    /// is worth nothing).
    pub fn new(coins_6000: f64) -> Self {
        TimePrice(coins_6000.max(0.0).to_bits())
    }

    pub fn coins_per_tick(self) -> f64 {
        f64::from_bits(self.0) / crate::guide_model::WINDOW_TICKS
    }
}

/// How a chain comes by what it sources (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sourcing {
    /// The cheaper of farming (ticks at this price) and buying (money
    /// chains, D16 revised 2026-11-16).
    Cheapest(TimePrice),
    /// Farmed whenever possible; an input nothing makes is missing, a
    /// consumable nothing makes is bought (autonomous training, D13).
    Farm,
}

/// True for a flow a chain or an autonomous line sources rather than
/// prices: a recipe input or a consumable of the method spent (decision
/// 2026-11-16 §1), never a fee or coins (money is what is paid).
pub fn sourceable(item: &str, cause: &str, qty: f64) -> bool {
    qty < 0.0 && item != "coins" && cause != "fee" && (cause == "input" || crate::guide_engine::BOUGHT_CAUSES.contains(&cause))
}

/// Causes under which an activity yields an object besides its product
/// legs (a by-product, a drop).
const YIELD_CAUSES: [&str; 2] = ["byproduct", "loot"];

/// One unit of a chain node's product.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainUnit {
    pub method: String,
    pub product: String,
    /// Ticks for one unit, upstream included.
    pub ticks_per_unit: f64,
    /// Coins of one unit when it is consumed downstream (every other
    /// flow of the node and its upstream, its product not sold).
    pub value_consumed: f64,
    /// XP per unit, per skill, upstream included.
    pub xp_per_unit: Vec<(String, f64)>,
    /// Stages upstream first, this one last.
    pub stages: Vec<Stage>,
    pub prereqs: Prereqs,
    /// Some stage rests on a measure.
    pub measured: bool,
    /// Inputs bought (no source, or buying is cheaper), per unit.
    pub bought: Vec<(String, f64)>,
    /// Purchases nobody covers (no source at the level, no ask deep
    /// enough, no shop), with the reason: the chain cannot run.
    pub missing: Vec<String>,
    /// The way each sourced input and consumable came by, upstream
    /// included (per unit of the consuming stage's product).
    pub routes: Vec<InputRoute>,
}

impl ChainUnit {
    /// Coins one unit costs when farmed by this source for a bot whose
    /// time is worth `price` (its ticks, less its other flows).
    pub fn farm_cost(&self, price: TimePrice) -> f64 {
        self.ticks_per_unit * price.coins_per_tick() - self.value_consumed
    }

    /// Coins of one unit sold at this stage when `units` of it are sold
    /// per window (a never-traded product's bids hold few units).
    pub fn value_sold_at(&self, engine: &Engine, units: f64) -> f64 {
        self.value_consumed + engine.sale_at(&self.product, units).price
    }
}

fn add_xp(into: &mut Vec<(String, f64)>, skill: &str, xp: f64) {
    match into.iter_mut().find(|(known, _)| known == skill) {
        Some(known) => known.1 += xp,
        None => into.push((skill.to_string(), xp)),
    }
}

/// Planner activities that can make `item` (its production legs keyed by
/// activity, trades aside), with the style their fight is read in.
pub fn sources(item: &str) -> Vec<(String, String)> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<(String, String)>>>> = std::sync::OnceLock::new();
    crate::guide_methods::memo(&MEMO, item.to_string(), || compute_sources(item))
}

fn compute_sources(item: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for leg in crate::obtain_legs::production_legs(item) {
        if ["market:", "shop:", "dialogue:"].iter().any(|prefix| leg.method.starts_with(prefix)) {
            continue;
        }
        let activity = crate::activity::activity_of_method(&leg.method);
        let style = if activity.starts_with("fight:melee/") { SOURCE_FIGHT_STYLE.to_string() } else { String::new() };
        if !out.iter().any(|(known, _)| *known == activity) {
            out.push((activity, style));
        }
    }
    for (activity, style) in yielders().iter().filter(|(_, _, items)| items.iter().any(|known| known == item)).map(|(activity, style, _)| (activity, style)) {
        if !out.iter().any(|(known, _)| known == activity) {
            out.push((activity.clone(), style.clone()));
        }
    }
    out
}

/// Activities of the catalogue with the objects their guide prior yields
/// as a by-product or a drop (`YIELD_CAUSES`), read at their gate level
/// with its tool tier; a fight's drops are read once, in melee
/// (`SOURCE_FIGHT_STYLE`, as a drop leg's fight). Built once per process
/// (pure function of the static game data).
fn yielders() -> &'static [(String, String, Vec<String>)] {
    static YIELDERS: std::sync::OnceLock<Vec<(String, String, Vec<String>)>> = std::sync::OnceLock::new();
    YIELDERS.get_or_init(|| {
        let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
        for entry in crate::guide_methods::catalogue() {
            if entry.method.starts_with("fight:") && !(entry.method.starts_with("fight:melee/") && entry.style == SOURCE_FIGHT_STYLE) {
                continue;
            }
            if out.iter().any(|(known, _, _)| *known == entry.method) {
                continue;
            }
            let level = crate::guide_methods::gate_level(&entry.method);
            let query = crate::method_rate::PriorQuery { method: entry.method.clone(), level, tool: crate::guide_methods::tool_at(&entry.method, &entry.style, level), site: None };
            let Ok(prior) = crate::method_rate::guide_prior(&query) else { continue };
            let mut items: Vec<String> = Vec::new();
            for (item, _, _) in prior.flows.iter().filter(|(_, cause, qty)| YIELD_CAUSES.contains(cause) && *qty > 0.0) {
                if !items.contains(item) {
                    items.push(item.clone());
                }
            }
            if !items.is_empty() {
                out.push((entry.method.clone(), entry.style.clone(), items));
            }
        }
        out
    })
}

/// The way one sourced flow comes by: farmed by a source, bought, or
/// neither (the reason).
pub enum Choice {
    Farm(ChainUnit),
    Buy(crate::guide_engine::Priced),
    Missing(String),
}

/// How `need` units (per unit of the consumer) of a sourced flow
/// (`sourceable`) come by under `sourcing`; `window_units` is the volume
/// a window of the consumer's work buys (D15 prices for it). Returns the
/// choice and its route.
#[allow(clippy::too_many_arguments)]
pub fn choose(engine: &Engine, item: &str, cause: &str, need: f64, window_units: f64, level: u8, depth: usize, downstream: &[String], sourcing: Sourcing) -> (Choice, InputRoute) {
    let input = cause == "input";
    let farmed = if depth < MAX_CHAIN_DEPTH { best_source(engine, item, level, depth + 1, downstream, sourcing) } else { None };
    let buyable = !(input && sourcing == Sourcing::Farm);
    let bought = if buyable { Some(engine.buy_at(item, window_units)).filter(|priced| priced.available) } else { None };
    let farm_coins = match (&farmed, sourcing) {
        (Some(source), Sourcing::Cheapest(price)) => Some(source.farm_cost(price)),
        _ => None,
    };
    let mut route = InputRoute {
        item: item.to_string(),
        cause: cause.to_string(),
        route: Route::Farm,
        source: String::new(),
        units: need,
        farm_ticks_per_unit: farmed.as_ref().map(|source| source.ticks_per_unit),
        farm_coins_per_unit: farm_coins,
        buy_coins_per_unit: bought.map(|priced| priced.price),
    };
    let buy_wins = match (&farmed, bought) {
        (None, Some(_)) => true,
        (Some(_), Some(priced)) => farm_coins.is_some_and(|farm| priced.price < farm),
        _ => false,
    };
    if let (true, Some(priced)) = (buy_wins, bought) {
        route.route = Route::Buy;
        return (Choice::Buy(priced), route);
    }
    if let Some(source) = farmed {
        route.source = source.method.clone();
        return (Choice::Farm(source), route);
    }
    route.route = Route::Buy;
    let reason = if input && sourcing == Sourcing::Farm {
        if depth >= MAX_CHAIN_DEPTH {
            format!("{item}: deeper than {MAX_CHAIN_DEPTH} stages, not farmed")
        } else {
            // Named only: the top line explains (`autonomous_gap`).
            format!("{item}: no autonomous source at level {level}")
        }
    } else {
        format!("{} (nothing makes it at level {level})", engine.unavailable_reason(item, window_units))
    };
    (Choice::Missing(reason), route)
}

/// One unit of `product` made by `method` for a bot at `level` in every
/// skill; `path` holds the objects downstream (no cycle). `Err` when the
/// method cannot make it at that level. Every input and consumable is
/// sourced by `choose` (module docs).
#[allow(clippy::too_many_arguments)]
pub fn chain_unit(engine: &Engine, method: &str, style: &str, product: &str, level: u8, depth: usize, path: &[String], sourcing: Sourcing) -> Result<ChainUnit, String> {
    if crate::guide_methods::gate_level(method) > level {
        return Err(format!("{method} needs level {}", crate::guide_methods::gate_level(method)));
    }
    engine.count("chain_unit");
    let tool = crate::guide_methods::tool_at(method, style, level);
    let ask = RateAsk { method: method.to_string(), style: style.to_string(), site: None, tool: tool.clone(), level };
    let answer = engine.answer(&ask)?;
    let rate = answer.gained(product);
    if rate <= 0.0 {
        return Err(format!("{method} yields no {product}"));
    }
    let own_ticks = (crate::guide_model::WINDOW_TICKS + answer.pause_per_6000.max(0.0)) / rate;
    let mut ticks = own_ticks;
    let mut value = 0.0;
    let mut xp: Vec<(String, f64)> = Vec::new();
    let mut stages: Vec<Stage> = Vec::new();
    let mut prereqs = crate::guide_prereq::prereqs_of(method, style, level, &tool);
    let mut measured = answer.confidence.measured;
    let mut bought: Vec<(String, f64)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut routes: Vec<InputRoute> = Vec::new();
    let mut downstream: Vec<String> = path.to_vec();
    downstream.push(product.to_string());
    for (item, cause, qty) in &answer.flows {
        if item == product && *qty > 0.0 {
            continue;
        }
        let per_unit = qty / rate;
        if sourceable(item, cause, *qty) {
            let need = -per_unit;
            let (choice, route) = choose(engine, item, cause, need, -qty, level, depth, &downstream, sourcing);
            routes.push(route);
            match choice {
                Choice::Farm(source) => {
                    ticks += need * source.ticks_per_unit;
                    value += need * source.value_consumed;
                    for (skill, gained) in &source.xp_per_unit {
                        add_xp(&mut xp, skill, need * gained);
                    }
                    for stage in &source.stages {
                        if !stages.iter().any(|known| known.method == stage.method && known.product == stage.product) {
                            stages.push(stage.clone());
                        }
                    }
                    crate::guide_prereq::merge(&mut prereqs, &source.prereqs);
                    measured |= source.measured;
                    bought.extend(source.bought.iter().map(|(name, count)| (name.clone(), count * need)));
                    routes.extend(source.routes.iter().map(|upstream| InputRoute { units: upstream.units * need, ..upstream.clone() }));
                }
                Choice::Buy(priced) => {
                    value -= need * priced.price;
                    crate::guide_prereq::require_shop(&mut prereqs, priced.need);
                    // An input rides the chain line as a bought flow; a
                    // consumable's purchase is in the value only.
                    if cause == "input" {
                        bought.push((item.clone(), need));
                    }
                }
                Choice::Missing(reason) => missing.push(reason),
            }
            continue;
        }
        let priced = engine.flow_price(item, cause, *qty, InputPricing::Stock);
        if !priced.available {
            missing.push(engine.unavailable_reason(item, -qty));
        }
        // A by-product or drop sold to a shop: its condition (Q7).
        if *qty > 0.0 {
            crate::guide_prereq::require_shop(&mut prereqs, priced.need);
        }
        value += per_unit * priced.price;
    }
    let level_xp = crate::unit_xp::derived_xp(method, style, &answer.units, level);
    for (skill, gained) in level_xp {
        add_xp(&mut xp, &skill, gained / rate);
    }
    let unit = ChainUnit { method: method.to_string(), product: product.to_string(), ticks_per_unit: ticks, value_consumed: value, xp_per_unit: xp, stages: Vec::new(), prereqs, measured, bought, missing, routes };
    let sold = unit.value_sold_at(engine, crate::guide_model::WINDOW_TICKS / ticks);
    stages.push(Stage { method: method.to_string(), product: product.to_string(), ticks_per_unit: ticks, profit_6000: sold / ticks * crate::guide_model::WINDOW_TICKS });
    Ok(ChainUnit { stages, ..unit })
}

/// Best source of one unit of `item` at `level`, `None` when none can
/// make it (a source with a purchase nobody covers cannot): the fewest
/// ticks per unit under `Sourcing::Farm`, the cheapest farmed unit (its
/// ticks at the bot's price, less its other flows) under
/// `Sourcing::Cheapest`.
pub fn best_source(engine: &Engine, item: &str, level: u8, depth: usize, path: &[String], sourcing: Sourcing) -> Option<ChainUnit> {
    if path.iter().any(|known| known == item) {
        return None;
    }
    engine.source_memo((item.to_string(), level, depth, sourcing), || compute_best_source(engine, item, level, depth, path, sourcing))
}

fn compute_best_source(engine: &Engine, item: &str, level: u8, depth: usize, path: &[String], sourcing: Sourcing) -> Option<ChainUnit> {
    let cost = |unit: &ChainUnit| match sourcing {
        Sourcing::Farm => unit.ticks_per_unit,
        Sourcing::Cheapest(price) => unit.farm_cost(price),
    };
    sources(item)
        .into_iter()
        .filter_map(|(method, style)| chain_unit(engine, &method, &style, item, level, depth, path, sourcing).ok())
        .filter(|unit| unit.missing.is_empty())
        .min_by(|left, right| cost(left).total_cmp(&cost(right)))
}

/// Why no autonomous source yields `item` at `level`: each source and
/// what stops it (its gate, no yield, an input it would have to buy).
pub fn autonomous_gap(engine: &Engine, item: &str, level: u8, depth: usize, path: &[String]) -> String {
    let found = sources(item);
    if found.is_empty() {
        return format!("{item}: no activity makes, farms or drops it (autonomous)");
    }
    if path.iter().any(|known| known == item) {
        return format!("{item}: made from itself downstream (autonomous)");
    }
    let mut reasons: Vec<String> = Vec::new();
    for (method, style) in found.iter().take(3) {
        let reason = match chain_unit(engine, method, style, item, level, depth, path, Sourcing::Farm) {
            Err(error) => error,
            Ok(unit) => format!("{method}: {}", unit.missing.first().cloned().unwrap_or_else(|| "no source".to_string())),
        };
        reasons.push(reason);
    }
    let more = if found.len() > 3 { format!(", {} more", found.len() - 3) } else { String::new() };
    format!("{item}: no autonomous source at level {level} ({}{more})", reasons.join("; "))
}
