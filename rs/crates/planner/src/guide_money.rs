//! Money making guide (plan rendements R4, §2, D5/D6/D12/D15/D16): one
//! line per F2P activity × site × tool × band × family, profit per 6000
//! ticks at the CURRENT price = Σ signed flows × price, every tick the
//! line costs counted (pauses, market trips, market waits, the chain's
//! upstream work).
//!
//! Families:
//! - farm: an activity without stock inputs (harvest, fight, pickup,
//!   cast): products sold (market or shop, Q3), consumables bought;
//! - recipe: transform owned stock, inputs at their sale price (D5);
//! - chain: farm the inputs then transform (D16, `guide_chain`), each
//!   stage's sell-here profit beside the line;
//! - market: buy every input (current price, or a shop's when cheaper,
//!   Q7), craft, sell (D15): ticks = work + a Varrock square round trip
//!   per lot + the measured market wait per lot; capital = one lot;
//!   Ironman excluded.
//!
//! Rows: per activity, the prior's best site and the tool tier of the
//! level, plus one row per measured exact key of that band (its site and
//! tool). XP per 6000 ticks is a bonus column (derived from the units).

use crate::guide_engine::{price_flows, round_trip_ticks, stackable, work_share, Engine, InputPricing};
use crate::guide_model::{band_of, Confidence, Family, GuideFilter, MoneyRow, RateAnswer, RateAsk, Unrated, LOT_SLOTS, VARROCK_SQUARE, WINDOW_TICKS};

/// One evaluated place of an activity: the ask and its yield.
pub struct Place {
    pub ask: RateAsk,
    pub answer: RateAnswer,
}

/// Level a row is read at: the filter's, else the method's own gate.
pub fn row_level(method: &str, filter: &GuideFilter) -> u8 {
    filter.level.unwrap_or_else(|| crate::guide_methods::gate_level(method)).clamp(1, 99)
}

/// Places of one activity at `level`: the prior's best site and tool,
/// then every measured exact key of the band (its own site and tool).
pub fn places(engine: &Engine, method: &str, style: &str, level: u8) -> (Vec<Place>, Option<String>) {
    let band = band_of(level);
    let mut asks = vec![RateAsk { method: method.to_string(), style: style.to_string(), site: None, tool: crate::guide_methods::tool_at(method, style, level), level }];
    for key in engine.world.measured.iter().filter(|key| key.method == method && key.style == style && key.band == band && key.site.is_some()) {
        asks.push(RateAsk { method: method.to_string(), style: style.to_string(), site: key.site, tool: key.tool.clone(), level });
    }
    let mut out = Vec::new();
    let mut error = None;
    for ask in asks {
        match engine.answer(&ask) {
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
        confidence: place.answer.confidence.clone(),
        accessible: false,
    }
}

/// Farm or recipe line of a place: its own flows, its pauses in time.
fn own_row(engine: &Engine, place: &Place, family: Family, band: i64) -> MoneyRow {
    let own = crate::guide_methods::own_product(&place.ask.method);
    let extra = place.answer.pause_per_6000.max(0.0);
    let scale = work_share(extra);
    let (flows, other, revenue, cost) = price_flows(engine, &place.answer, own.as_deref(), InputPricing::Stock, scale, &|_, _| false);
    let mut row = base_row(family, place, band);
    row.flows = flows;
    row.other_drops = other;
    row.revenue_6000 = revenue;
    row.cost_6000 = cost;
    row.profit_6000 = revenue - cost;
    row.xp_6000 = xp_scaled(&place.ask.method, &place.ask.style, &place.answer, place.ask.level, scale);
    row.extra_ticks_per_6000 = extra;
    row
}

/// True when every stock input of a place can be bought (a fill, an ask
/// or a shop): else no 100 % market line exists.
pub fn inputs_buyable(engine: &Engine, place: &Place) -> bool {
    place.answer.flows.iter().filter(|(_, cause, qty)| cause == "input" && *qty < 0.0).all(|(item, _, _)| engine.buy(item).available)
}

/// 100 % market line (D15): inputs bought, a square round trip and the
/// measured wait per lot (a pack of non-stackable inputs).
pub fn market_row(engine: &Engine, place: &Place, band: i64) -> MoneyRow {
    let answer = &place.answer;
    let slots_per_6000: f64 = answer.flows.iter().filter(|(item, cause, qty)| cause == "input" && *qty < 0.0 && !stackable(item)).map(|(_, _, qty)| -qty).sum();
    let lots_per_6000 = slots_per_6000 / LOT_SLOTS;
    let trip = workshop(place).and_then(|site| round_trip_ticks(VARROCK_SQUARE, site)).unwrap_or(0.0);
    let wait = engine.world.market_wait_ticks.unwrap_or(0.0);
    let extra = answer.pause_per_6000.max(0.0) + lots_per_6000 * (trip + wait);
    let scale = work_share(extra);
    let own = crate::guide_methods::own_product(&place.ask.method);
    let (flows, other, revenue, cost) = price_flows(engine, answer, own.as_deref(), InputPricing::Bought, scale, &|_, _| false);
    let mut row = base_row(crate::guide_model::Family::Market, place, band);
    let inputs_per_6000: f64 = answer.flows.iter().filter(|(_, cause, qty)| cause == "input" && *qty < 0.0).map(|(item, _, qty)| -qty * engine.buy(item).price).sum();
    // One lot ties up its inputs: a pack of them (all stackable: the
    // inputs of one window).
    row.prereqs.capital = if lots_per_6000 > 0.0 { inputs_per_6000 / lots_per_6000 } else { inputs_per_6000 };
    row.prereqs.ironman = false;
    row.flows = flows;
    row.other_drops = other;
    row.revenue_6000 = revenue;
    row.cost_6000 = cost;
    row.profit_6000 = revenue - cost;
    row.xp_6000 = xp_scaled(&place.ask.method, &place.ask.style, answer, place.ask.level, scale);
    row.extra_ticks_per_6000 = extra;
    if engine.world.market_wait_ticks.is_none() {
        row.confidence.read = format!("{} (market wait unmeasured)", row.confidence.read);
    }
    row
}

/// Chain line (D16) of an activity at the prior's place.
fn chain_row(engine: &Engine, place: &Place, band: i64) -> Option<MoneyRow> {
    let product = crate::guide_methods::own_product(&place.ask.method)?;
    let unit = crate::guide_chain::chain_unit(engine, &place.ask.method, &place.ask.style, &product, place.ask.level, 0, &[]).ok()?;
    // A chain farms at least one input: else it is the recipe line.
    if unit.stages.len() < 2 {
        return None;
    }
    let units_per_6000 = WINDOW_TICKS / unit.ticks_per_unit;
    let sold = unit.value_sold(engine);
    let mut row = base_row(Family::Chain, place, band);
    row.prereqs = unit.prereqs.clone();
    let priced = engine.sale(&product);
    row.flows = vec![crate::guide_model::FlowLine { item: product.clone(), cause: "product".to_string(), per_6000: units_per_6000, unit_price: priced.price, value_6000: units_per_6000 * priced.price, outlet: priced.outlet, traded: priced.traded }];
    for (item, per_unit) in &unit.bought {
        let bought = engine.buy(item);
        row.flows.push(crate::guide_model::FlowLine { item: item.clone(), cause: "input".to_string(), per_6000: -per_unit * units_per_6000, unit_price: bought.price, value_6000: -per_unit * units_per_6000 * bought.price, outlet: bought.outlet, traded: bought.traded });
    }
    row.profit_6000 = sold * units_per_6000;
    row.revenue_6000 = units_per_6000 * priced.price;
    row.cost_6000 = row.revenue_6000 - row.profit_6000;
    row.xp_6000 = unit.xp_per_unit.iter().map(|(skill, xp)| (skill.clone(), xp * units_per_6000)).collect();
    row.extra_ticks_per_6000 = unit.ticks_per_unit * units_per_6000 - WINDOW_TICKS;
    row.stages = unit.stages;
    row.confidence.measured = unit.measured;
    Some(row)
}

/// Every money line of one activity at `level` (filter applied later).
pub fn method_rows(engine: &Engine, method: &str, style: &str, level: u8) -> (Vec<MoneyRow>, Option<Unrated>) {
    let band = band_of(level);
    let (found, error) = places(engine, method, style, level);
    if found.is_empty() {
        return (Vec::new(), Some(Unrated { method: method.to_string(), reason: error.unwrap_or_else(|| "no yield".to_string()) }));
    }
    let mut rows = Vec::new();
    for (index, place) in found.iter().enumerate() {
        if !has_stock_inputs(&place.answer) {
            rows.push(own_row(engine, place, Family::Farm, band));
            continue;
        }
        rows.push(own_row(engine, place, Family::Recipe, band));
        if inputs_buyable(engine, place) {
            rows.push(market_row(engine, place, band));
        }
        // The chain farms upstream at the prior's places: one per band.
        if index == 0 {
            rows.extend(chain_row(engine, place, band));
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

/// The money making guide under `filter`, best profit first, and the
/// activities nothing estimates.
pub fn money_guide(engine: &Engine, filter: &GuideFilter) -> (Vec<MoneyRow>, Vec<Unrated>) {
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
        let (found, missing) = method_rows(engine, &entry.method, &entry.style, level);
        unrated.extend(missing.filter(|gap| !unrated.iter().any(|known: &Unrated| known.method == gap.method)));
        for mut row in found {
            if filter.family.as_deref().is_some_and(|family| family != row.family.as_str()) {
                continue;
            }
            row.accessible = crate::guide_prereq::passes(&row.prereqs, filter);
            rows.push(row);
        }
    }
    rows.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000));
    (rows, unrated)
}

/// Profit per 6000 ticks of the best ACCESSIBLE line for a bot at
/// `level` with `quests_done` (D17's coin -> time rate), `None` when no
/// accessible line earns.
pub fn best_accessible_profit(engine: &Engine, level: u8, quests_done: &[String]) -> Option<f64> {
    let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    let (rows, _) = money_guide(engine, &filter);
    rows.iter().filter(|row| row.accessible && row.profit_6000 > 0.0).map(|row| row.profit_6000).fold(None, |best: Option<f64>, profit| Some(best.map_or(profit, |known| known.max(profit))))
}

/// Confidence of a fabricated or missing answer (tests, quest rows).
pub fn unmeasured() -> Confidence {
    Confidence { read: "PriorOnly".to_string(), ..Confidence::default() }
}

#[cfg(test)]
#[path = "guide_tests.rs"]
mod tests;
