//! Money making guide (plan rendements R4, §2, D5/D6/D12/D15/D16): one
//! line per F2P activity × site × tool × band × family, profit per 6000
//! ticks at the CURRENT price = Σ signed flows × price, every tick the
//! line costs counted (pauses, market trips, market waits, the chain's
//! upstream work). Every flow is priced for the volume it moves
//! (`guide_engine`: a never-traded object's book counts only for its
//! depth).
//!
//! Families:
//! - farm: an activity without stock inputs (harvest, fight, pickup,
//!   cast): products sold (market or shop, Q3), consumables bought;
//! - recipe: transform owned stock, inputs at their sale price (D5).
//!   Only a bot HOLDING the stock can work it: the row names one lot of
//!   stock as a prerequisite; in the general guide (nothing held) it is
//!   blocked and ranks after every other row; for a bot (filter `held`)
//!   it is evaluated on its stock (the window's work its stock covers);
//! - chain: source the inputs then transform (D16 revised 2026-11-16,
//!   `guide_chain`): each input the cheaper way, farmed (its ticks at the
//!   bot's D17 price) or bought (D15 at its volume), the way of each on
//!   the line (`routes`), each stage's sell-here profit beside it. The
//!   D17 price a chain reads is the best accessible line OUTSIDE the
//!   chains (a chain's own rate depends on that choice: the loop is cut
//!   there);
//! - market: buy every input (current price, or a shop's when cheaper,
//!   Q7), craft, sell (D15): ticks = work + a Varrock square round trip
//!   per lot + the measured market wait per lot; capital = one lot;
//!   Ironman excluded.
//!
//! An input nothing makes at the level, no ask covers and no shop sells
//! blocks the chain or market row (reason given, no profit). Blocked rows
//! are never accessible and rank after every other row.
//!
//! Rows: per activity, the prior's best site and the tool tier of the
//! level, plus one row per measured exact key of that band (its site and
//! tool) whose OWN key carries the measure (decision 2026-11-16 §4: a
//! measured site the estimator reads through a roll-up or the prior is
//! no line, never a copy; a key listed twice is one line). XP per 6000
//! ticks is a bonus column (derived from the units).

use crate::guide_chain::{Sourcing, TimePrice};
use crate::guide_engine::{lots_per_6000, price_flows, round_trip_ticks, stock_inputs, stock_lot, work_share, Engine, InputPricing, PricedFlows};
use crate::guide_model::{band_of, price_confidence, Confidence, Family, GuideFilter, MoneyRow, RateAnswer, RateAsk, Unrated, VARROCK_SQUARE, WINDOW_TICKS};

/// One evaluated place of an activity: the ask and its yield.
pub struct Place {
    pub ask: RateAsk,
    pub answer: RateAnswer,
}

/// Level a row is read at: the filter's, else the method's own gate.
pub fn row_level(method: &str, filter: &GuideFilter) -> u8 {
    filter.level.unwrap_or_else(|| crate::guide_methods::gate_level(method)).clamp(1, 99)
}

/// The estimator's read of an exact key (`Confidence::read`).
pub const EXACT_READ: &str = "Exact";

/// Places of one activity at `level`: the prior's best site and tool,
/// then each measured exact key of the band (its own site and tool),
/// once, when its own key carries the measure (§4: a site read through
/// a roll-up or the prior would copy another line).
pub fn places(engine: &Engine, method: &str, style: &str, level: u8) -> (Vec<Place>, Option<String>) {
    let band = band_of(level);
    let mut asks = vec![RateAsk { method: method.to_string(), style: style.to_string(), site: None, tool: crate::guide_methods::tool_at(method, style, level), level }];
    for key in engine.world.measured.iter().filter(|key| key.method == method && key.style == style && key.band == band && key.site.is_some()) {
        if asks.iter().any(|known| known.site == key.site && known.tool == key.tool) {
            continue;
        }
        asks.push(RateAsk { method: method.to_string(), style: style.to_string(), site: key.site, tool: key.tool.clone(), level });
    }
    let mut out = Vec::new();
    let mut error = None;
    for ask in asks {
        match engine.answer(&ask) {
            Ok(answer) if ask.site.is_some() && answer.confidence.read != EXACT_READ => {}
            Ok(answer) => out.push(Place { ask, answer }),
            Err(reason) => error = Some(reason),
        }
    }
    (out, error)
}

fn xp_scaled(method: &str, style: &str, answer: &RateAnswer, level: u8, scale: f64) -> Vec<(String, f64)> {
    crate::unit_xp::derived_xp(method, style, &answer.units, level).into_iter().map(|(skill, xp)| (skill, xp * scale)).collect()
}

/// Workshop of a place: its site, else the prior's best site; `None`
/// for work done anywhere (at the square's own bank).
fn workshop(place: &Place) -> Option<(i64, i64, i64)> {
    place.ask.site.or_else(|| crate::method_rate::best_site(&crate::method_rate::PriorQuery { method: place.ask.method.clone(), level: place.ask.level, tool: place.ask.tool.clone(), site: None }))
}

fn has_stock_inputs(answer: &RateAnswer) -> bool {
    answer.flows.iter().any(|(_, cause, qty)| cause == "input" && *qty < 0.0)
}

fn base_row(family: Family, place: &Place, band: i64) -> MoneyRow {
    MoneyRow {
        family,
        method: place.ask.method.clone(),
        style: place.ask.style.clone(),
        site: place.ask.site,
        tool: place.ask.tool.clone(),
        band,
        prereqs: crate::guide_prereq::prereqs_of(&place.ask.method, &place.ask.style, place.ask.level, &place.ask.tool),
        flows: Vec::new(),
        other_drops: Default::default(),
        revenue_6000: 0.0,
        cost_6000: 0.0,
        profit_6000: 0.0,
        xp_6000: Vec::new(),
        extra_ticks_per_6000: 0.0,
        stages: Vec::new(),
        routes: Vec::new(),
        confidence: place.answer.confidence.clone(),
        price_confidence: "traded",
        blocked: Vec::new(),
        accessible: false,
    }
}

/// Fills a row's priced flows; a purchase nobody covers blocks the row
/// and leaves it without a profit (never a made-up price).
fn apply_flows(row: &mut MoneyRow, priced: PricedFlows) {
    crate::guide_prereq::require_shop(&mut row.prereqs, priced.need);
    row.price_confidence = price_confidence(&priced.lines);
    row.flows = priced.lines;
    row.other_drops = priced.other;
    if priced.missing.is_empty() {
        row.revenue_6000 = priced.revenue;
        row.cost_6000 = priced.cost;
        row.profit_6000 = priced.revenue - priced.cost;
    } else {
        row.blocked.extend(priced.missing);
    }
}

/// Units of `item` a bot holds.
fn held_of(held: &[(String, f64)], item: &str) -> f64 {
    held.iter().filter(|(name, _)| name == item).fold(0.0, |total, (_, units)| total + units)
}

/// Farm or recipe line of a place: its own flows, its pauses in time. A
/// recipe names one lot of its stock; nothing held (the general guide)
/// blocks it, priced on a full window; a bot's stock (`held`) evaluates
/// it on the share of the window that stock covers.
fn own_row(engine: &Engine, place: &Place, family: Family, band: i64, held: &[(String, f64)]) -> MoneyRow {
    let own = crate::guide_methods::own_product(&place.ask.method);
    let extra = place.answer.pause_per_6000.max(0.0);
    let share = work_share(extra);
    let mut row = base_row(family, place, band);
    let mut coverage = 1.0;
    if family == Family::Recipe {
        row.prereqs.stock = stock_lot(&place.answer);
        let lacking: Vec<String> = row.prereqs.stock.iter().filter(|(item, lot)| held_of(held, item) < *lot).map(|(item, lot)| format!("{lot:.0} {item} (held {:.0})", held_of(held, item))).collect();
        if !lacking.is_empty() {
            row.blocked.push(format!("stock: one lot needs {}", lacking.join(", ")));
        }
        if !held.is_empty() {
            coverage = stock_inputs(&place.answer).iter().map(|(item, units)| (held_of(held, item) / (units * share)).min(1.0)).fold(1.0, f64::min);
        }
    }
    let scale = share * coverage;
    apply_flows(&mut row, price_flows(engine, &place.answer, own.as_deref(), InputPricing::Stock, scale, &|_, _| false));
    row.xp_6000 = xp_scaled(&place.ask.method, &place.ask.style, &place.answer, place.ask.level, scale);
    row.extra_ticks_per_6000 = extra;
    row
}

/// 100 % market line (D15): inputs bought, a square round trip and the
/// measured wait per lot (a pack of non-stackable inputs). An input no
/// ask covers and no shop sells blocks it.
pub fn market_row(engine: &Engine, place: &Place, band: i64) -> MoneyRow {
    let answer = &place.answer;
    let lots = lots_per_6000(answer);
    let trip = workshop(place).and_then(|site| round_trip_ticks(VARROCK_SQUARE, site)).unwrap_or(0.0);
    let wait = engine.world.market_wait_ticks.unwrap_or(0.0);
    let extra = answer.pause_per_6000.max(0.0) + lots * (trip + wait);
    let scale = work_share(extra);
    let own = crate::guide_methods::own_product(&place.ask.method);
    let priced = price_flows(engine, answer, own.as_deref(), InputPricing::Bought, scale, &|_, _| false);
    let mut row = base_row(crate::guide_model::Family::Market, place, band);
    // One lot ties up its inputs: a pack of them (all stackable: the
    // inputs of one window of work).
    let inputs_per_6000: f64 = priced.lines.iter().filter(|line| line.cause == "input" && line.per_6000 < 0.0).map(|line| -line.value_6000).sum::<f64>() / scale;
    row.prereqs.capital = if lots > 0.0 { inputs_per_6000 / lots } else { inputs_per_6000 };
    row.prereqs.ironman = false;
    apply_flows(&mut row, priced);
    row.xp_6000 = xp_scaled(&place.ask.method, &place.ask.style, answer, place.ask.level, scale);
    row.extra_ticks_per_6000 = extra;
    if engine.world.market_wait_ticks.is_none() {
        row.confidence.read = format!("{} (market wait unmeasured)", row.confidence.read);
    }
    row
}

/// Chain line (D16 revised) of an activity at the prior's place, its
/// inputs sourced the cheaper way for a bot whose time is worth `time`.
fn chain_row(engine: &Engine, place: &Place, band: i64, time: TimePrice) -> Option<MoneyRow> {
    let product = crate::guide_methods::own_product(&place.ask.method)?;
    let unit = crate::guide_chain::chain_unit(engine, &place.ask.method, &place.ask.style, &product, place.ask.level, 0, &[], Sourcing::Cheapest(time)).ok()?;
    // A chain farms at least one input: else it is the recipe line.
    if unit.stages.len() < 2 {
        return None;
    }
    let units_per_6000 = WINDOW_TICKS / unit.ticks_per_unit;
    let mut row = base_row(Family::Chain, place, band);
    row.prereqs = unit.prereqs.clone();
    let priced = engine.sale_at(&product, units_per_6000);
    crate::guide_prereq::require_shop(&mut row.prereqs, priced.need);
    row.flows = vec![crate::guide_model::FlowLine {
        item: product.clone(),
        cause: "product".to_string(),
        per_6000: units_per_6000,
        unit_price: priced.price,
        value_6000: units_per_6000 * priced.price,
        outlet: priced.outlet,
        traded: priced.traded,
        basis: priced.basis,
    }];
    let mut missing = unit.missing.clone();
    for (item, per_unit) in &unit.bought {
        let units = per_unit * units_per_6000;
        let bought = engine.buy_at(item, units);
        if !bought.available {
            missing.push(engine.unavailable_reason(item, units));
        }
        crate::guide_prereq::require_shop(&mut row.prereqs, bought.need);
        row.flows.push(crate::guide_model::FlowLine { item: item.clone(), cause: "input".to_string(), per_6000: -units, unit_price: bought.price, value_6000: -units * bought.price, outlet: bought.outlet, traded: bought.traded, basis: bought.basis });
    }
    row.price_confidence = price_confidence(&row.flows);
    if missing.is_empty() {
        row.profit_6000 = (unit.value_consumed + priced.price) * units_per_6000;
        row.revenue_6000 = units_per_6000 * priced.price;
        row.cost_6000 = row.revenue_6000 - row.profit_6000;
    } else {
        missing.dedup();
        row.blocked = missing;
    }
    row.xp_6000 = unit.xp_per_unit.iter().map(|(skill, xp)| (skill.clone(), xp * units_per_6000)).collect();
    row.extra_ticks_per_6000 = unit.ticks_per_unit * units_per_6000 - WINDOW_TICKS;
    row.stages = unit.stages;
    row.routes = unit.routes;
    row.confidence.measured = unit.measured;
    Some(row)
}

/// Every money line of one activity at `level` (filter applied later),
/// recipes evaluated on `held` (empty: nothing held); `chains`: the
/// quests of the bot whose D17 price the chain line reads, `None` for no
/// chain line (the lines that set that price).
pub fn method_rows(engine: &Engine, method: &str, style: &str, level: u8, held: &[(String, f64)], chains: Option<&[String]>) -> (Vec<MoneyRow>, Option<Unrated>) {
    let band = band_of(level);
    let (found, error) = places(engine, method, style, level);
    if found.is_empty() {
        return (Vec::new(), Some(Unrated { method: method.to_string(), reason: error.unwrap_or_else(|| "no yield".to_string()) }));
    }
    let mut rows = Vec::new();
    for (index, place) in found.iter().enumerate() {
        if !has_stock_inputs(&place.answer) {
            rows.push(own_row(engine, place, Family::Farm, band, held));
            continue;
        }
        rows.push(own_row(engine, place, Family::Recipe, band, held));
        rows.push(market_row(engine, place, band));
        // The chain farms upstream at the prior's places: one per band.
        if let (0, Some(quests)) = (index, chains) {
            let time = TimePrice::new(best_profit_outside_chains(engine, level, quests).unwrap_or(0.0));
            rows.extend(chain_row(engine, place, band, time));
        }
    }
    (rows, None)
}

/// Activities of a build: the catalogue plus every measured activity it
/// does not list (an alchemy of a gathered object, a style a fight
/// was measured in), never the melee ladder (its rungs are listed).
pub fn entries(engine: &Engine) -> Vec<crate::guide_methods::GuideMethod> {
    let mut out: Vec<crate::guide_methods::GuideMethod> = crate::guide_methods::catalogue().to_vec();
    for key in engine.world.measured {
        if key.method == crate::guide_methods::LADDER || out.iter().any(|entry| entry.method == key.method && entry.style == key.style) {
            continue;
        }
        out.push(crate::guide_methods::GuideMethod { method: key.method.clone(), style: key.style.clone() });
    }
    out
}

/// The money making guide under `filter`: unblocked rows best profit
/// first, then the blocked ones (best profit first), and the activities
/// nothing estimates.
pub fn money_guide(engine: &Engine, filter: &GuideFilter) -> (Vec<MoneyRow>, Vec<Unrated>) {
    guide_rows(engine, filter, true)
}

fn guide_rows(engine: &Engine, filter: &GuideFilter, with_chains: bool) -> (Vec<MoneyRow>, Vec<Unrated>) {
    let mut rows = Vec::new();
    let mut unrated = Vec::new();
    for entry in &entries(engine) {
        let level = row_level(&entry.method, filter);
        if filter.level.is_some() && crate::guide_methods::gate_level(&entry.method) > level {
            continue;
        }
        if let Some(skill) = &filter.skill {
            if crate::activity::band_skill(&entry.method, &entry.style).as_deref() != Some(skill.as_str()) {
                continue;
            }
        }
        let chains = with_chains.then_some(filter.quests_done.as_slice());
        let (found, missing) = method_rows(engine, &entry.method, &entry.style, level, &filter.held, chains);
        unrated.extend(missing.filter(|gap| !unrated.iter().any(|known: &Unrated| known.method == gap.method)));
        for mut row in found {
            if filter.family.as_deref().is_some_and(|family| family != row.family.as_str()) {
                continue;
            }
            row.accessible = row.blocked.is_empty() && crate::guide_prereq::passes(&row.prereqs, filter);
            rows.push(row);
        }
    }
    // Usable first (not blocked, prerequisites met), then by value.
    rows.sort_by(|left, right| (left.blocked.is_empty() && left.accessible).cmp(&(right.blocked.is_empty() && right.accessible)).reverse().then(right.profit_6000.total_cmp(&left.profit_6000)));
    (rows, unrated)
}

/// Profit per 6000 ticks of the best ACCESSIBLE line for a bot at
/// `level` with `quests_done` (D17's coin -> time rate), `None` when no
/// accessible line earns. Nothing held: a recipe on stock never sets it.
pub fn best_accessible_profit(engine: &Engine, level: u8, quests_done: &[String]) -> Option<f64> {
    let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    let (rows, _) = money_guide(engine, &filter);
    rows.iter().filter(|row| row.accessible && row.profit_6000 > 0.0).map(|row| row.profit_6000).fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
}

/// D17 price a chain reads (§3): the best ACCESSIBLE line outside the
/// chains for a bot at `level` with `quests_done`, built once per build;
/// `None` when none earns (time is then worth nothing).
pub fn best_profit_outside_chains(engine: &Engine, level: u8, quests_done: &[String]) -> Option<f64> {
    engine.best_profit_memo((level, quests_done.to_vec(), false), || {
        let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
        let (rows, _) = guide_rows(engine, &filter, false);
        rows.iter().filter(|row| row.accessible && row.profit_6000 > 0.0).map(|row| row.profit_6000).fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
    })
}

/// Confidence of a fabricated or missing answer (tests, quest rows).
pub fn unmeasured() -> Confidence {
    Confidence { read: "PriorOnly".to_string(), ..Confidence::default() }
}

#[cfg(test)]
#[path = "guide_tests.rs"]
mod tests;
