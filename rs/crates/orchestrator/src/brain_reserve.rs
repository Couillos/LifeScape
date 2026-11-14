#![forbid(unsafe_code)]
//! Seller reserve = the bot's OWN cost of one unit (user rule 1,
//! 2026-11-14). It replaces the half-reference floor that ratcheted down
//! with every dump (willow 13 -> 6 -> 2 -> 1) and the 1-coin default:
//!
//! `reserve(item) = ticks one unit cost THIS bot x THIS bot's coins per tick`
//!
//! Every input is the bot's own data (its task reports, its own fills),
//! never another bot's state:
//! - ticks per unit, measured: each own report's ticks are shared among
//!   its farm lines (`produced`, `loot`, `ground_spawn`, looted coins) in
//!   proportion to `quantity x object value` (the game's own item value,
//!   coins at 1): a gathering or production report gives its product
//!   all its ticks; only `produced` units take a share (drops are found,
//!   see below).
//!   A processing report (forge, smelt, cook…) also names the inputs it
//!   ate (`consumed`): their quantity per unit, shared the same way, is
//!   costed at the input's OWN unit cost (its measured ticks x coins per
//!   tick plus its own inputs, 3 levels deep, else its cold-start ticks,
//!   and never under its own purchase price): iron full helm = its ticks
//!   + 2 iron bars, an iron bar = its smelting ticks + its iron ore.
//!   Unmeasured: the planner's estimate of the way that makes it (the
//!   `est_ticks` of a farm alternative of an obtain node of the bot's
//!   graph, per unit), else the planner's cold-start rate of the
//!   gathering script (`rates_service::prior_for`). Nothing at all: no
//!   time cost.
//! - coins per tick: the best of the bot's own methods, measured as
//!   (coins looted or shop-sold + units it farmed x its OWN last sale
//!   price of them) / ticks, never under `market_fit::MIN_COIN_RATE`
//!   (0.1: never free time, the floor `bid_response` prices with).
//! - a unit it BOUGHT costs at least its own last purchase price.
//! - never over what MAKING it costs the bot as `bid_response` prices a
//!   bid quota (`bid_response::production_cost`: coins + ticks at the
//!   bot's coin rate, `brain_bids::production_costs_for`): a quota counts
//!   the bids above that cost, so a producer never holds the stock the
//!   bids it answered would buy.
//!
//! FOUND stock (2026-11-14, live frank_lane: 41 iron daggers, 88 bronze
//! med helms, 838 bronze arrows banked, one dagger listed at 628 for a
//! value of ~30): loot, drops and ground spawns cost the bot NOTHING. The
//! fight paid itself in XP (or coins), the walk to a spawn in the spawn;
//! sharing the fight's ticks among the drops priced a steel arrow at
//! 3541. An object the bot's own reports only show as found (or that a
//! combat prior alone would cost) gets a `Found` reserve of 1 coin: it is
//! surplus, sold at a clearing price (`lifescape_market::found_open`).
//! Made and found units of one object: the made share keeps its time.
//! No own record at all: the `bid_response` make cost when the bot can
//! make it, else found.

use lifescape_planner::{Life, NodeKind, NodeStatus};
use lifescape_store::{Store, TaskReport, TaskYield};

/// Newest own reports read per pass (its current level and site).
pub const OWN_REPORTS: i64 = 200;

/// Input levels costed under one unit (helm → bar → ore).
pub const INPUT_DEPTH: u8 = 3;

/// One farm or coin line of an own report (display name).
#[derive(Clone, Debug, PartialEq)]
pub struct OwnLine {
    pub item: String,
    pub provenance: String,
    pub quantity: i64,
}

/// One own task report: its script, ticks and item lines.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnReport {
    pub method: String,
    pub ticks: i64,
    pub lines: Vec<OwnLine>,
}

/// Where a reserve came from (log line and tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostSource {
    Measured,
    Planned,
    Prior,
    Bought,
    /// Capped at the `bid_response` cost of making it.
    MakeCost,
    /// Loot, drop or ground spawn: zero own cost, 1-coin floor.
    Found,
}

/// A seller reserve in coins per unit, with its source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reserve {
    pub coins: i64,
    pub source: CostSource,
}

/// Coins display name (reports carry display names).
pub fn is_coins(name: &str) -> bool {
    name.eq_ignore_ascii_case("coins")
}

fn farm_line(line: &OwnLine) -> bool {
    line.quantity > 0 && crate::rates_service::is_farm_provenance(&line.provenance)
}

/// Provenances of FOUND stock (loot, drops, ground spawns): zero own cost.
pub fn is_found_provenance(provenance: &str) -> bool {
    matches!(provenance, "loot" | "ground_spawn" | "drop")
}

/// A line that MADE units (gathered, processed): they carry its ticks.
fn made_line(line: &OwnLine) -> bool {
    line.quantity > 0 && line.provenance == "produced"
}

/// Units of `display` the bot's counted reports MADE and FOUND.
pub fn own_origin(reports: &[OwnReport], display: &str) -> (i64, i64) {
    let mut made = 0i64;
    let mut found = 0i64;
    for line in reports.iter().filter(|report| counted(report)).flat_map(|report| report.lines.iter()) {
        if line.quantity <= 0 || !line.item.eq_ignore_ascii_case(display) {
            continue;
        }
        if line.provenance == "produced" {
            made += line.quantity;
        } else if is_found_provenance(&line.provenance) {
            found += line.quantity;
        }
    }
    (made, found)
}

fn coin_gain(line: &OwnLine) -> bool {
    // `produced` coins: alchemy turned stock into coins.
    line.quantity > 0 && crate::brain_reserve::is_coins(&line.item) && matches!(line.provenance.as_str(), "loot" | "shop_sold" | "produced")
}

/// Input line of a processing report: what the run ate (`consumed`).
fn input_line(line: &OwnLine) -> bool {
    line.quantity < 0 && line.provenance == "consumed" && !crate::brain_reserve::is_coins(&line.item)
}

/// Each counted report that made `display`: the units it made and the
/// report's share owed to them (`quantity x weight` among its farm lines
/// and coin gains). Found units (loot, ground spawns) take no share of
/// their own; they still weigh in the total (a fight's drops never make
/// its other products dearer).
fn unit_shares<'a>(reports: &'a [OwnReport], display: &str, weight: &dyn Fn(&str) -> f64) -> Vec<(&'a OwnReport, i64, f64)> {
    let mut shares = Vec::new();
    for report in reports.iter().filter(|report| counted(report)) {
        let own: i64 = report.lines.iter().filter(|line| made_line(line) && line.item.eq_ignore_ascii_case(display)).map(|line| line.quantity).sum();
        if own <= 0 {
            continue;
        }
        let total: f64 = report
            .lines
            .iter()
            .filter(|line| farm_line(line) || coin_gain(line))
            .map(|line| line.quantity as f64 * if crate::brain_reserve::is_coins(&line.item) { 1.0 } else { weight(&line.item).max(1.0) })
            .sum();
        if total <= 0.0 {
            continue;
        }
        shares.push((report, own, own as f64 * weight(display).max(1.0) / total));
    }
    shares
}

fn counted(report: &OwnReport) -> bool {
    report.ticks > 0 && !crate::rates_service::is_trading_method(&report.method)
}

/// Measured ticks one MADE unit of `display` cost: the report ticks
/// shared by `quantity x weight` among the report's farm lines and coin
/// gains. `None` for found-only objects (loot is free).
pub fn measured_ticks_per_unit(reports: &[OwnReport], display: &str, weight: &dyn Fn(&str) -> f64) -> Option<f64> {
    let shares = unit_shares(reports, display, weight);
    let units: i64 = shares.iter().map(|(_, own, _)| *own).sum();
    let ticks: f64 = shares.iter().map(|(report, _, share)| report.ticks as f64 * share).sum();
    (units > 0).then(|| ticks / units as f64)
}

/// Measured inputs one unit of `display` ate: the `consumed` lines of the
/// reports that made it, shared like their ticks, per unit made (display
/// names, sorted). Empty for gathered or looted objects.
pub fn measured_inputs_per_unit(reports: &[OwnReport], display: &str, weight: &dyn Fn(&str) -> f64) -> Vec<(String, f64)> {
    let shares = unit_shares(reports, display, weight);
    let units: i64 = shares.iter().map(|(_, own, _)| *own).sum();
    if units <= 0 {
        return Vec::new();
    }
    let mut inputs: Vec<(String, f64)> = Vec::new();
    for (report, _, share) in &shares {
        for line in report.lines.iter().filter(|line| input_line(line)) {
            let eaten = -line.quantity as f64 * share / units as f64;
            match inputs.iter_mut().find(|(item, _)| item.eq_ignore_ascii_case(&line.item)) {
                Some(known) => known.1 += eaten,
                None => inputs.push((line.item.clone(), eaten)),
            }
        }
    }
    inputs.sort_by(|left, right| left.0.cmp(&right.0));
    inputs
}

/// Best measured coins per tick over the bot's own methods: coins
/// looted or shop-sold, plus the units it farmed (net of what it sold to
/// a shop in that report) at its own last sale price. `None` below one
/// coin earned.
pub fn measured_coins_per_tick(reports: &[OwnReport], own_sale: &dyn Fn(&str) -> Option<i64>) -> Option<f64> {
    let mut by_method: Vec<(String, f64, i64)> = Vec::new();
    for report in reports.iter().filter(|report| counted(report)) {
        let mut coins = 0.0;
        for line in &report.lines {
            if coin_gain(line) {
                coins += line.quantity as f64;
            } else if farm_line(line) && !crate::brain_reserve::is_coins(&line.item) {
                let shop_sold: i64 = report
                    .lines
                    .iter()
                    .filter(|other| other.provenance == "shop_sold" && other.quantity < 0 && other.item.eq_ignore_ascii_case(&line.item))
                    .map(|other| -other.quantity)
                    .sum();
                let kept = (line.quantity - shop_sold).max(0);
                coins += kept as f64 * own_sale(&line.item).unwrap_or(0).max(0) as f64;
            } else if input_line(line) {
                // A processing run nets its inputs at their own sale
                // price: forging bars is worth the helms minus the bars.
                coins += line.quantity as f64 * own_sale(&line.item).unwrap_or(0).max(0) as f64;
            }
        }
        match by_method.iter_mut().find(|(method, _, _)| *method == report.method) {
            Some(entry) => {
                entry.1 += coins;
                entry.2 += report.ticks;
            }
            None => by_method.push((report.method.clone(), coins, report.ticks)),
        }
    }
    by_method
        .into_iter()
        .filter(|(_, coins, ticks)| *coins >= 1.0 && *ticks > 0)
        .map(|(_, coins, ticks)| coins / ticks as f64)
        .fold(None, |best: Option<f64>, rate| Some(best.map_or(rate, |known| known.max(rate))))
}

/// The bot's coins per tick: its best measured method, never under the
/// shared floor `market_fit::MIN_COIN_RATE` (one tick is never free).
pub fn coins_per_tick(reports: &[OwnReport], own_sale: &dyn Fn(&str) -> Option<i64>) -> f64 {
    measured_coins_per_tick(reports, own_sale).unwrap_or(0.0).max(lifescape_planner::market_fit::MIN_COIN_RATE)
}

/// Planner estimate from the bot's own graph: the cheapest farm way of an
/// obtain node of `item` (debug name), its `est_ticks` per unit.
pub fn planned_ticks_per_unit(life: &Life, item: &str) -> Option<f64> {
    let mut best: Option<f64> = None;
    for node in &life.nodes {
        let NodeKind::ObtainItems { item: wanted, count } = &node.kind else { continue };
        if wanted != item || *count <= 0 || matches!(node.status, NodeStatus::Abandoned) {
            continue;
        }
        for alt in &node.alternatives {
            let trade = ["market:", "shop:", "dialogue:"].iter().any(|prefix| alt.method.starts_with(prefix));
            if trade || alt.est_ticks == 0 || alt.est_ticks >= u64::MAX / 4 {
                continue;
            }
            let per_unit = alt.est_ticks as f64 / *count as f64;
            best = Some(best.map_or(per_unit, |known: f64| known.min(per_unit)));
        }
    }
    best
}

/// Planner cold-start estimate of the gathering script that makes
/// `display` (a combat prior is loot: found, no time cost).
pub fn prior_ticks_per_unit(display: &str) -> Option<f64> {
    ["fishing", "woodcut", "mining"]
        .iter()
        .filter_map(|script| crate::rates_service::prior_for(script, display))
        .map(|(per_6000, _)| per_6000)
        .filter(|per_6000| *per_6000 > 0.0)
        .map(|per_6000| crate::rates_service::NORMALIZE_TICKS / per_6000)
        .next()
}

/// Coins per unit of `ticks_per_unit` at `coins_per_tick` plus the
/// inputs' coins, rounded up, at least 1 coin.
pub fn unit_coins(ticks_per_unit: f64, coins_per_tick: f64, inputs: f64) -> i64 {
    let cost = ticks_per_unit.max(0.0) * coins_per_tick.max(0.0) + inputs.max(0.0);
    (cost.ceil() as i64).max(1)
}

/// Coins per unit of `ticks_per_unit` at `coins_per_tick`, rounded up,
/// at least 1 coin.
pub fn reserve_coins(ticks_per_unit: f64, coins_per_tick: f64) -> i64 {
    unit_coins(ticks_per_unit, coins_per_tick, 0.0)
}

/// The reserve from the bot's own figures: the time cost (measured,
/// planned, prior) plus the coins of the inputs one unit ate (`inputs`,
/// measured, 0 for a gathered object), and its own last purchase price,
/// the larger wins, then never over `make_cost` (the `bid_response` cost
/// of making one unit, rounded down, at least 1 coin).
pub fn reserve_of(measured: Option<f64>, planned: Option<f64>, prior: Option<f64>, coins_per_tick: f64, inputs: f64, bought: Option<i64>, make_cost: Option<f64>) -> Option<Reserve> {
    let time = measured
        .map(|ticks| (ticks, CostSource::Measured))
        .or(planned.map(|ticks| (ticks, CostSource::Planned)))
        .or(prior.map(|ticks| (ticks, CostSource::Prior)))
        .map(|(ticks, source)| Reserve { coins: unit_coins(ticks, coins_per_tick, inputs), source });
    let paid = bought.filter(|price| *price >= 1).map(|price| Reserve { coins: price, source: CostSource::Bought });
    let own = match (time, paid) {
        (Some(time), Some(paid)) => Some(if paid.coins > time.coins { paid } else { time }),
        (time, paid) => time.or(paid),
    }?;
    let cap = make_cost.filter(|cost| cost.is_finite()).map(|cost| (cost.floor() as i64).max(1));
    Some(match cap {
        Some(cap) if cap < own.coins => Reserve { coins: cap, source: CostSource::MakeCost },
        _ => own,
    })
}

/// Reserve of an object without own time cost: its own last purchase
/// (capped like any reserve), else `Found` when the bot found it (or
/// knows nothing of it and cannot make it), else the make cost.
pub fn found_reserve(found_units: i64, bought: Option<i64>, make_cost: Option<f64>) -> Reserve {
    if let Some(paid) = reserve_of(None, None, None, 0.0, 0.0, bought, make_cost) {
        return paid;
    }
    match make_cost.filter(|cost| cost.is_finite() && found_units <= 0) {
        Some(cost) => Reserve { coins: (cost.floor() as i64).max(1), source: CostSource::MakeCost },
        None => Reserve { coins: 1, source: CostSource::Found },
    }
}

/// Display name of a debug name (yields carry display names).
pub fn display_of(item: &str) -> String {
    lifescape_gamedata::objs_by_name(item).into_iter().next().and_then(|obj| obj.display.or(obj.name)).unwrap_or(item).to_string()
}

/// The base (unnoted, tradeable) object of a display name.
fn obj_of_display(display: &str) -> Option<&'static lifescape_gamedata::Obj> {
    lifescape_gamedata::all_objs().iter().find(|obj| obj.tradeable && obj.display.is_some_and(|name| name.eq_ignore_ascii_case(display)))
}

/// Game value of one unit, the loot-share weight (at least 1).
pub fn item_weight(display: &str) -> f64 {
    obj_of_display(display).map(|obj| obj.price.max(1) as f64).unwrap_or(1.0)
}

/// The account's own last purchase price of a display name.
fn last_bought(store: &Store, account: &str, display: &str) -> Option<f64> {
    let id = obj_of_display(display)?.obj_id as i64;
    store.last_own_fill(account, id, true).ok().flatten().map(|(price, _)| price as f64)
}

fn own_reports(rows: Vec<(TaskReport, Vec<TaskYield>)>) -> Vec<OwnReport> {
    rows.into_iter()
        .map(|(report, lines)| OwnReport {
            method: report.method,
            ticks: report.ticks,
            lines: lines
                .into_iter()
                .filter(|line| !line.item_name.is_empty())
                .map(|line| OwnLine { item: line.item_name, provenance: line.provenance, quantity: line.quantity })
                .collect(),
        })
        .collect()
}

/// One account's own cost figures, loaded once per selling pass.
pub struct OwnCosts {
    pub account: String,
    pub reports: Vec<OwnReport>,
    pub coins_per_tick: f64,
}

impl OwnCosts {
    /// Reads the account's own reports and own sale prices.
    pub fn load(store: &Store, account: &str) -> OwnCosts {
        let reports = own_reports(store.account_reports(account, OWN_REPORTS).unwrap_or_default());
        let own_sale = |display: &str| {
            let id = obj_of_display(display)?.obj_id as i64;
            store.last_own_fill(account, id, false).ok().flatten().map(|(price, _)| price)
        };
        let coins_per_tick = coins_per_tick(&reports, &own_sale);
        OwnCosts { account: account.to_string(), reports, coins_per_tick }
    }

    /// Reserve of `item` (debug name, obj id `item_id`), capped at
    /// `make_cost` (`brain_bids::production_costs_for`) when known.
    pub fn reserve(&self, store: &Store, item: &str, item_id: i64, life: Option<&Life>, make_cost: Option<f64>) -> Option<Reserve> {
        let display = display_of(item);
        let bought = store.last_own_fill(&self.account, item_id, true).ok().flatten().map(|(price, _)| price);
        let (made, found) = own_origin(&self.reports, &display);
        if made <= 0 {
            // Found, or nothing measured: a planned or gathering prior
            // only for an object the bot never found.
            let planned = life.filter(|_| found <= 0).and_then(|life| planned_ticks_per_unit(life, item));
            let prior = if found <= 0 { prior_ticks_per_unit(&display) } else { None };
            if planned.is_none() && prior.is_none() {
                return Some(found_reserve(found, bought, make_cost));
            }
            return reserve_of(None, planned, prior, self.coins_per_tick, 0.0, bought, make_cost);
        }
        // Made and found: the found units cost nothing, the made share
        // keeps its time and inputs.
        let made_share = made as f64 / (made + found.max(0)) as f64;
        let measured = measured_ticks_per_unit(&self.reports, &display, &item_weight).map(|ticks| ticks * made_share);
        let inputs = self.input_coins(&|input: &str| last_bought(store, &self.account, input), &display, INPUT_DEPTH) * made_share;
        reserve_of(measured, None, None, self.coins_per_tick, inputs, bought, make_cost)
    }

    /// Coins of the inputs one unit of `display` ate, each at its own
    /// unit cost (`unit_cost`); `bought(input)` = its own last purchase.
    pub fn input_coins(&self, bought: &dyn Fn(&str) -> Option<f64>, display: &str, depth: u8) -> f64 {
        if depth == 0 {
            return 0.0;
        }
        measured_inputs_per_unit(&self.reports, display, &item_weight)
            .iter()
            .map(|(input, per_unit)| per_unit * self.unit_cost(bought, input, depth - 1))
            .sum()
    }

    /// Own unit cost of an input (display name): its measured ticks at the
    /// bot's rate plus its own inputs, else its cold-start ticks, never
    /// under its own last purchase price; 0 when nothing is known.
    pub fn unit_cost(&self, bought: &dyn Fn(&str) -> Option<f64>, display: &str, depth: u8) -> f64 {
        if matches!(own_origin(&self.reports, display), (0, found) if found > 0) {
            // A found input (looted feathers) cost no time.
            return bought(display).unwrap_or(0.0);
        }
        let made = measured_ticks_per_unit(&self.reports, display, &item_weight)
            .map(|ticks| ticks * self.coins_per_tick + self.input_coins(bought, display, depth))
            .or_else(|| prior_ticks_per_unit(display).map(|ticks| ticks * self.coins_per_tick));
        match (made, bought(display)) {
            (Some(made), Some(paid)) => made.max(paid),
            (made, paid) => made.or(paid).unwrap_or(0.0),
        }
    }
}

#[cfg(test)]
#[path = "brain_reserve_tests.rs"]
mod tests;
