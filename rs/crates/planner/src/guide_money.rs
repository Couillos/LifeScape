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
//!   stock as a condition (D22: information, `guide_feasible` judges it
//!   per bot); in the general guide (nothing held) it is priced on a full
//!   window; for a bot (filter `held`) it is evaluated on its stock (the
//!   window's work its stock covers);
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
//! D22 (user 2026-11-17): no row is ever blocked. A purchase the asks
//! and the shops do not cover is priced at the current price
//! (`guide_engine`) and named in the row's conditions (`MarketNeed`:
//! asks, recent fills, shops, units per lot); an object nothing prices
//! (never traded, no ask, no shop) or a chain input nothing makes nor
//! prices leaves the row unpriced (`priced` false, no profit, ranked
//! after the priced rows), its conditions saying why. Each bot judges
//! a row by `guide_feasible::feasible_for`.
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
    for key in engine.measured_sites(method, style, band) {
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
        priced: true,
    }
}

/// Fills a row's priced flows and the availability of what it buys (its
/// conditions, D22); a purchase nothing prices leaves the row unpriced
/// (no profit, never a made-up price).
fn apply_flows(row: &mut MoneyRow, priced: PricedFlows) {
    crate::guide_prereq::require_shop(&mut row.prereqs, priced.need);
    row.price_confidence = price_confidence(&priced.lines);
    row.flows = priced.lines;
    row.other_drops = priced.other;
    row.prereqs.market.extend(priced.market);
    row.priced = priced.priced;
    if priced.priced {
        row.revenue_6000 = priced.revenue;
        row.cost_6000 = priced.cost;
        row.profit_6000 = priced.revenue - priced.cost;
    }
}

/// Units of `item` a bot holds.
fn held_of(held: &[(String, f64)], item: &str) -> f64 {
    held.iter().filter(|(name, _)| name == item).fold(0.0, |total, (_, units)| total + units)
}

/// Farm or recipe line of a place: its own flows, its pauses in time. A
/// recipe names one lot of its stock (a condition, D22); nothing held
/// (the general guide) prices it on a full window; a bot's stock
/// (`held`) evaluates it on the share of the window that stock covers.
fn own_row(engine: &Engine, place: &Place, family: Family, band: i64, held: &[(String, f64)]) -> MoneyRow {
    let own = crate::guide_methods::own_product(&place.ask.method);
    let extra = place.answer.pause_per_6000.max(0.0);
    let share = work_share(extra);
    let mut row = base_row(family, place, band);
    let mut coverage = 1.0;
    if family == Family::Recipe {
        row.prereqs.stock = stock_lot(&place.answer);
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
/// measured wait per lot (a pack of non-stackable inputs). What the asks
/// and the shops hold of each input is a condition (D22), never a block.
pub fn market_row(engine: &Engine, place: &Place, band: i64) -> MoneyRow {
    market_row_with(engine, place, band, None)
}

/// `market_row` whose purchases carry `per_purchase` slots of inputs each
/// (the training market line, user lever 2026-11-17: a climb buys its
/// tranche's inputs at once, need bids at the tranche's volume, one
/// square round trip and one market wait per purchase, the bank trips
/// between packs already in the yield); `None` or under one pack: one
/// purchase per pack (D15's lot). The capital stays one pack's.
pub fn market_row_with(engine: &Engine, place: &Place, band: i64, per_purchase: Option<f64>) -> MoneyRow {
    let answer = &place.answer;
    let lots = lots_per_6000(answer);
    let purchases = match per_purchase {
        Some(slots) if slots > crate::guide_model::LOT_SLOTS => lots * crate::guide_model::LOT_SLOTS / slots,
        _ => lots,
    };
    let trip = workshop(place).and_then(|site| round_trip_ticks(VARROCK_SQUARE, site)).unwrap_or(0.0);
    let wait = engine.world.market_wait_ticks.unwrap_or(0.0);
    let extra = answer.pause_per_6000.max(0.0) + purchases * (trip + wait);
    let scale = work_share(extra);
    let own = crate::guide_methods::own_product(&place.ask.method);
    let priced = price_flows(engine, answer, own.as_deref(), InputPricing::Bought, scale, &|_, _| false);
    let mut row = base_row(crate::guide_model::Family::Market, place, band);
    // One lot ties up its inputs: a pack of them (all stackable: the
    // inputs of one window of work).
    let inputs_per_6000: f64 = priced.lines.iter().filter(|line| line.cause == "input" && line.per_6000 < 0.0).map(|line| -line.value_6000).sum::<f64>() / scale;
    row.prereqs.capital = if lots > 0.0 { inputs_per_6000 / lots } else { inputs_per_6000 };
    row.prereqs.purchase_ticks = trip + wait;
    row.prereqs.purchases_per_6000 = purchases * scale;
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
    row.prereqs.sources.extend(unit.gaps.iter().cloned());
    row.priced = unit.gaps.is_empty();
    // One lot: a pack of the non-stackable inputs bought, else the window.
    let slots: f64 = unit.bought.iter().filter(|(item, cause, _)| cause == "input" && !crate::guide_engine::stackable(item)).map(|(_, _, per_unit)| per_unit * units_per_6000).sum();
    let lots = if slots > 0.0 { slots / crate::guide_model::LOT_SLOTS } else { 1.0 };
    for (item, cause, per_unit) in &unit.bought {
        let units = per_unit * units_per_6000;
        let bought = engine.buy_at(item, units);
        row.priced &= bought.priced;
        crate::guide_prereq::require_shop(&mut row.prereqs, bought.need);
        row.prereqs.market.push(engine.market_need(item, cause, units, crate::guide_engine::whole_units(units / lots), bought.priced));
        // A consumable's purchase rides the unit's value only.
        if cause == "input" {
            row.flows.push(crate::guide_model::FlowLine { item: item.clone(), cause: cause.clone(), per_6000: -units, unit_price: bought.price, value_6000: -units * bought.price, outlet: bought.outlet, traded: bought.traded, basis: bought.basis });
        }
    }
    row.price_confidence = price_confidence(&row.flows);
    if row.priced {
        row.profit_6000 = (unit.value_consumed + priced.price) * units_per_6000;
        row.revenue_6000 = units_per_6000 * priced.price;
        row.cost_6000 = row.revenue_6000 - row.profit_6000;
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
/// chain line (the lines that set that price). `recipes` receives the
/// place of each recipe row (its index among the rows returned); `None`
/// leaves the recipe rows out (the D17 pass: nothing held, a recipe on
/// stock is blocked, never accessible, it never sets the price).
pub fn method_rows(engine: &Engine, method: &str, style: &str, level: u8, held: &[(String, f64)], chains: Option<&[String]>, mut recipes: Option<&mut Vec<(usize, RateAsk, RateAnswer)>>) -> (Vec<MoneyRow>, Option<Unrated>) {
    let band = band_of(level);
    let (found, error) = engine.timed("places", || places(engine, method, style, level));
    if found.is_empty() {
        return (Vec::new(), Some(Unrated { method: method.to_string(), reason: error.unwrap_or_else(|| "no yield".to_string()) }));
    }
    let mut rows = Vec::new();
    for (index, place) in found.iter().enumerate() {
        if !has_stock_inputs(&place.answer) {
            rows.push(engine.timed("own_row", || own_row(engine, place, Family::Farm, band, held)));
            continue;
        }
        if let Some(recipes) = recipes.as_deref_mut() {
            recipes.push((rows.len(), place.ask.clone(), place.answer.clone()));
            rows.push(engine.timed("own_row", || own_row(engine, place, Family::Recipe, band, held)));
        }
        rows.push(engine.timed("market_row", || market_row(engine, place, band)));
        // The chain farms upstream at the prior's places: one per band.
        if let (0, Some(quests)) = (index, chains) {
            let time = TimePrice::new(best_profit_outside_chains(engine, level, quests).unwrap_or(0.0));
            rows.extend(engine.timed("chain_row", || chain_row(engine, place, band, time)));
        }
    }
    (rows, None)
}

/// The recipe row of a place re-evaluated on a bot's stock `held` (R7:
/// `guide_read`, the view's other rows unchanged), at the view's level.
pub fn recipe_row(engine: &Engine, ask: &RateAsk, answer: &RateAnswer, held: &[(String, f64)]) -> MoneyRow {
    let place = Place { ask: ask.clone(), answer: answer.clone() };
    own_row(engine, &place, Family::Recipe, band_of(ask.level), held)
}

/// Activities of a build: the catalogue plus every measured activity it
/// does not list (an alchemy of a gathered object, a style a fight
/// was measured in), never the melee ladder (its rungs are listed).
pub fn entries<'e>(engine: &'e Engine) -> &'e [crate::guide_methods::GuideMethod] {
    engine.entries_memo(|| list_entries(engine))
}

fn list_entries(engine: &Engine) -> Vec<crate::guide_methods::GuideMethod> {
    let mut out: Vec<crate::guide_methods::GuideMethod> = crate::guide_methods::catalogue().to_vec();
    let mut listed: std::collections::HashSet<(String, String)> = out.iter().map(|entry| (entry.method.clone(), entry.style.clone())).collect();
    for key in engine.world.measured {
        if key.method == crate::guide_methods::LADDER || !listed.insert((key.method.clone(), key.style.clone())) {
            continue;
        }
        out.push(crate::guide_methods::GuideMethod { method: key.method.clone(), style: key.style.clone() });
    }
    out
}

/// The money making guide under `filter`: unblocked rows best profit
/// first, then the blocked ones (best profit first), and the activities
/// nothing estimates (`guide_read::finish_money` of a fresh view).
pub fn money_guide(engine: &Engine, filter: &GuideFilter) -> (Vec<MoneyRow>, Vec<Unrated>) {
    let view = money_view(engine, filter.level, &filter.quests_done, &filter.held);
    crate::guide_read::finish_money(&view, filter, &std::collections::HashMap::new())
}

/// Every money row of a bot at `level` (`None`: each activity at its
/// gate) with `quests_done` holding `held`, chain lines included,
/// activity order (`guide_read`: the reader's filter applies to it).
pub fn money_view(engine: &Engine, level: Option<u8>, quests_done: &[String], held: &[(String, f64)]) -> crate::guide_read::MoneyView {
    engine.count("money_guide");
    let filter = GuideFilter { level, quests_done: quests_done.to_vec(), held: held.to_vec(), ..GuideFilter::default() };
    let mut rows: Vec<MoneyRow> = Vec::new();
    let mut gaps = Vec::new();
    let mut recipes: Vec<crate::guide_read::RecipePlace> = Vec::new();
    for entry in entries(engine) {
        let level = row_level(&entry.method, &filter);
        if filter.level.is_some() && crate::guide_methods::gate_level(&entry.method) > level {
            continue;
        }
        let mut places_of: Vec<(usize, RateAsk, RateAnswer)> = Vec::new();
        let (found, missing) = method_rows(engine, &entry.method, &entry.style, level, held, Some(quests_done), Some(&mut places_of));
        if let Some(gap) = missing {
            gaps.push(crate::guide_read::Gap { method: gap.method, style: entry.style.clone(), reason: gap.reason });
        }
        recipes.extend(places_of.into_iter().map(|(index, ask, answer)| crate::guide_read::RecipePlace { row: rows.len() + index, ask, answer }));
        rows.extend(found);
    }
    crate::guide_read::MoneyView { level, rows, gaps, recipes }
}

/// Profit per 6000 ticks of the best ACCESSIBLE line for a bot at
/// `level` with `quests_done` (D17's coin -> time rate), `None` when no
/// accessible line earns. Nothing held: a recipe on stock never sets it.
/// Built once per build (`seed_best_accessible_profit` sets it from a
/// view already built).
pub fn best_accessible_profit(engine: &Engine, level: u8, quests_done: &[String]) -> Option<f64> {
    engine.best_profit_memo((level, quests_done.to_vec(), true), || {
        engine.aside(|| {
            let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
            let view = money_view(engine, Some(level), quests_done, &[]);
            crate::guide_read::best_profit_in(&view, &filter)
        })
    })
}

/// Reads the yields of every activity a bot at `level` works (the
/// prior's place and the measured sites of its band) into the build's
/// memo: the views of that level and their D17 pass read them there.
pub fn read_level(engine: &Engine, level: u8) {
    for entry in entries(engine) {
        if crate::guide_methods::gate_level(&entry.method) <= level {
            let _ = places(engine, &entry.method, &entry.style, level);
        }
    }
}

/// Records the D17 price of a bot at `level` with `quests_done` from the
/// money view built for it (nothing held): the training guide of that
/// bot reads it instead of building the same view again.
pub fn seed_best_accessible_profit(engine: &Engine, view: &crate::guide_read::MoneyView, quests_done: &[String]) {
    let Some(level) = view.level else { return };
    let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
    engine.best_profit_memo((level, quests_done.to_vec(), true), || crate::guide_read::best_profit_in(view, &filter));
}

/// D17 price a chain reads (§3): the best ACCESSIBLE line outside the
/// chains for a bot at `level` with `quests_done`, built once per build;
/// `None` when none earns (time is then worth nothing). Recipe rows on
/// stock are left out of that pass: nothing held, no bot of the filter
/// works them (`guide_feasible`).
pub fn best_profit_outside_chains(engine: &Engine, level: u8, quests_done: &[String]) -> Option<f64> {
    engine.best_profit_memo((level, quests_done.to_vec(), false), || engine.aside(|| {
        engine.count("money_guide_no_chain");
        let filter = GuideFilter { level: Some(level), quests_done: quests_done.to_vec(), ..GuideFilter::default() };
        crate::guide_feasible::with_filter_bot(&filter, |bot| best_outside_chains_for(engine, level, &filter, bot))
    }))
}

fn best_outside_chains_for(engine: &Engine, level: u8, filter: &GuideFilter, bot: &crate::guide_feasible::BotState) -> Option<f64> {
    let mut best: Option<f64> = None;
    for entry in entries(engine) {
        let gate = crate::guide_methods::gate_level(&entry.method);
        if gate > level {
            continue;
        }
        // A row's prerequisites are its activity's (levels, combat,
        // quests: never its tool nor its yield) plus shop conditions
        // (`require_shop`, only more): an activity the bot cannot
        // work sets no price, its yields are never read here. Read at
        // its gate (one memo entry per activity): its skill's level
        // max(gate, level) and the gate both pass at `level`.
        let base = crate::guide_prereq::prereqs_of(&entry.method, &entry.style, gate, "");
        if !crate::guide_prereq::passes(&base, filter) {
            continue;
        }
        let (found, _) = method_rows(engine, &entry.method, &entry.style, level, &[], None, None);
        for row in found {
            if row.priced && row.profit_6000 > 0.0 && crate::guide_feasible::feasible_for(&row, bot).within_reach() {
                best = Some(best.map_or(row.profit_6000, |known: f64| known.max(row.profit_6000)));
            }
        }
    }
    best
}

/// Confidence of a fabricated or missing answer (tests, quest rows).
pub fn unmeasured() -> Confidence {
    Confidence { read: "PriorOnly".to_string(), ..Confidence::default() }
}

#[cfg(test)]
#[path = "guide_tests.rs"]
mod tests;
