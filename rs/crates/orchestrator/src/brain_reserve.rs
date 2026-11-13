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
//!   all its ticks, a combat report gives each drop its loot share.
//!   Unmeasured: the planner's estimate of the way that makes it (the
//!   `est_ticks` of a farm alternative of an obtain node of the bot's
//!   graph, per unit), else the planner's cold-start rate of the
//!   gathering/combat script (`rates_service::prior_for`). Nothing at
//!   all: no time cost.
//! - coins per tick: the best of the bot's own methods, measured as
//!   (coins looted or shop-sold + units it farmed x its OWN last sale
//!   price of them) / ticks. A bot that never looted coins also weighs
//!   the planner's estimate of the coin-drop method `have_coins` uses
//!   (men: coins per kill x the cold-start kill rate), the earning it
//!   has not tried yet.
//! - a unit it BOUGHT costs at least its own last purchase price.
//!
//! No time cost and no purchase: `None`, the bot holds the stock (never
//! a 1-coin listing).

use lifescape_planner::{Life, NodeKind, NodeStatus};
use lifescape_store::{Store, TaskReport, TaskYield};

/// Newest own reports read per pass (its current level and site).
pub const OWN_REPORTS: i64 = 200;

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

fn coin_gain(line: &OwnLine) -> bool {
    line.quantity > 0 && crate::brain_reserve::is_coins(&line.item) && matches!(line.provenance.as_str(), "loot" | "shop_sold")
}

fn counted(report: &OwnReport) -> bool {
    report.ticks > 0 && !crate::rates_service::is_trading_method(&report.method)
}

/// Measured ticks one unit of `display` cost: the report ticks shared
/// by `quantity x weight` among the report's farm lines and coin gains.
pub fn measured_ticks_per_unit(reports: &[OwnReport], display: &str, weight: &dyn Fn(&str) -> f64) -> Option<f64> {
    let mut ticks = 0.0;
    let mut units = 0i64;
    for report in reports.iter().filter(|report| counted(report)) {
        let own: i64 = report.lines.iter().filter(|line| farm_line(line) && line.item.eq_ignore_ascii_case(display)).map(|line| line.quantity).sum();
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
        ticks += report.ticks as f64 * own as f64 * weight(display).max(1.0) / total;
        units += own;
    }
    (units > 0).then(|| ticks / units as f64)
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

/// Planner estimate of the coin-drop method `have_coins` sends a bot to
/// (men): expected coins per kill x the cold-start kill rate (one bones
/// drop per kill, `prior_for("melee", "bones")`).
pub fn estimated_coin_drop_rate() -> f64 {
    let per_kill = lifescape_planner::resolve_obtain::coins_per_kill("fight:melee/men");
    let kills = crate::rates_service::prior_for("melee", "bones").map(|(per_6000, _)| per_6000).unwrap_or(0.0);
    per_kill * kills / crate::rates_service::NORMALIZE_TICKS
}

/// The bot's coins per tick: its best measured method, and the coin-drop
/// estimate while it never looted coins itself.
pub fn coins_per_tick(reports: &[OwnReport], own_sale: &dyn Fn(&str) -> Option<i64>) -> f64 {
    let measured = measured_coins_per_tick(reports, own_sale).unwrap_or(0.0);
    let looted = reports.iter().any(|report| report.lines.iter().any(|line| coin_gain(line) && line.provenance == "loot"));
    if looted {
        measured
    } else {
        measured.max(estimated_coin_drop_rate())
    }
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

/// Planner cold-start estimate of the script that farms `display`.
pub fn prior_ticks_per_unit(display: &str) -> Option<f64> {
    ["fishing", "woodcut", "mining", "melee"]
        .iter()
        .filter_map(|script| crate::rates_service::prior_for(script, display))
        .map(|(per_6000, _)| per_6000)
        .filter(|per_6000| *per_6000 > 0.0)
        .map(|per_6000| crate::rates_service::NORMALIZE_TICKS / per_6000)
        .next()
}

/// Coins per unit of `ticks_per_unit` at `coins_per_tick`, rounded up,
/// at least 1 coin.
pub fn reserve_coins(ticks_per_unit: f64, coins_per_tick: f64) -> i64 {
    let cost = ticks_per_unit.max(0.0) * coins_per_tick.max(0.0);
    (cost.ceil() as i64).max(1)
}

/// The reserve from the bot's own figures: the time cost (measured,
/// planned, prior) and its own last purchase price, the larger wins.
pub fn reserve_of(measured: Option<f64>, planned: Option<f64>, prior: Option<f64>, coins_per_tick: f64, bought: Option<i64>) -> Option<Reserve> {
    let time = measured
        .map(|ticks| (ticks, CostSource::Measured))
        .or(planned.map(|ticks| (ticks, CostSource::Planned)))
        .or(prior.map(|ticks| (ticks, CostSource::Prior)))
        .map(|(ticks, source)| Reserve { coins: reserve_coins(ticks, coins_per_tick), source });
    let paid = bought.filter(|price| *price >= 1).map(|price| Reserve { coins: price, source: CostSource::Bought });
    match (time, paid) {
        (Some(time), Some(paid)) => Some(if paid.coins > time.coins { paid } else { time }),
        (time, paid) => time.or(paid),
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

    /// Reserve of `item` (debug name, obj id `item_id`).
    pub fn reserve(&self, store: &Store, item: &str, item_id: i64, life: Option<&Life>) -> Option<Reserve> {
        let display = display_of(item);
        let measured = measured_ticks_per_unit(&self.reports, &display, &item_weight);
        let planned = life.and_then(|life| planned_ticks_per_unit(life, item));
        let prior = prior_ticks_per_unit(&display);
        let bought = store.last_own_fill(&self.account, item_id, true).ok().flatten().map(|(price, _)| price);
        reserve_of(measured, planned, prior, self.coins_per_tick, bought)
    }
}

#[cfg(test)]
#[path = "brain_reserve_tests.rs"]
mod tests;
