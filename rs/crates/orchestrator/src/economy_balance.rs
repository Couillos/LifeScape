#![forbid(unsafe_code)]
//! `orchestrator economy` section "équilibre (plan-equilibre)" (docs/
//! plan-equilibre.txt step 0, lot 0 of docs/plan-equilibre-dev.txt): the
//! hourly board of the self-balancing market, each figure beside its
//! 2026-11-19 baseline and its target (plan-equilibre-dev §M).
//!
//! - days of unsold stock per object: units resting in sell orders over
//!   the units the object traded per day in the window
//!   (`Store::unsold_stock`), top 15, the untraded objects, fleet total
//!   over every unit the window traded (`Store::window_traded_units`);
//! - fleet work time per skill: `task_reports.work_ticks` of the window
//!   (`Store::work_by_method`), each planner method mapped to the skill
//!   it trains or harvests by `planner::activity::band_skill` (combat
//!   style first, else the game-data way); work no skill gates is
//!   labelled by its method family;
//! - filler share: work of the filler nodes (`brain_filler::is_filler_node`);
//! - 100 % market lots: realized over predicted of the closed lots
//!   (`trade_report::closed_totals`), lots opened in the window and all,
//!   beside the lots still open (buying, crafting, selling).

use std::collections::BTreeMap;

use lifescape_store::{stock_days, MethodWork, Store, UnsoldStock};

use crate::trade_report::TradeRow;

/// Skills whose time plan-equilibre §M wants above 10 % together
/// (crafting, magic, cooking, runes: 3.2 % on 2026-11-19).
pub const TRANSFORM_SKILLS: [&str; 4] = ["crafting", "magic", "cooking", "runecraft"];
/// Product whose harvest plan-equilibre §M wants under 10 % of the time.
pub const YEW_PRODUCT: &str = "yew_logs";
/// Objects listed by days of stock.
const STOCK_TOP: usize = 15;

/// Fleet work time of a window, split as the plan measures it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkShares {
    pub total: i64,
    pub filler: i64,
    /// (skill or `(none) <family>`, work ticks), most worked first.
    pub by_skill: Vec<(String, i64)>,
    /// Work whose method's way yields yew logs.
    pub yew: i64,
}

impl WorkShares {
    /// `part` over the total (`None` without work).
    pub fn share(&self, part: i64) -> Option<f64> {
        (self.total > 0).then(|| part as f64 / self.total as f64)
    }

    /// Work ticks of the transformation skills (§M).
    pub fn transform(&self) -> i64 {
        self.by_skill.iter().filter(|(skill, _)| TRANSFORM_SKILLS.contains(&skill.as_str())).map(|(_, ticks)| *ticks).sum()
    }
}

/// Skill a planner method trains or harvests (`band_skill`), else
/// `(none) <family>` (market making, shop runs, ground pickups...).
pub fn skill_label(planner_method: &str, combat_style: &str) -> String {
    if planner_method.is_empty() {
        return "(none) unmeasured".to_string();
    }
    match lifescape_planner::activity::band_skill(planner_method, combat_style) {
        Some(skill) => skill,
        None => format!("(none) {}", planner_method.split([':', '/']).next().unwrap_or(planner_method)),
    }
}

/// True when the method's way yields yew logs (harvest or filler).
fn harvests_yew(planner_method: &str) -> bool {
    lifescape_planner::method_way::method_way(planner_method).is_some_and(|(product, _)| product == YEW_PRODUCT)
}

/// The window's work rows split by skill, filler and yew harvest.
pub fn work_shares(rows: &[MethodWork]) -> WorkShares {
    let mut shares = WorkShares::default();
    let mut by_skill: BTreeMap<String, i64> = BTreeMap::new();
    for row in rows {
        shares.total += row.work_ticks;
        if crate::brain_filler::is_filler_node(&row.node_id) {
            shares.filler += row.work_ticks;
        }
        if harvests_yew(&row.planner_method) {
            shares.yew += row.work_ticks;
        }
        *by_skill.entry(skill_label(&row.planner_method, &row.combat_style)).or_insert(0) += row.work_ticks;
    }
    shares.by_skill = by_skill.into_iter().collect();
    shares.by_skill.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    shares
}

/// Days of unsold stock: (traded objects by days, most first, at most
/// 15; untraded objects with asks by resting units, most first; fleet
/// days = all resting units over `window_traded`, every unit the window
/// traded per day, sold-out objects included).
pub fn stock_ranking(stock: &[UnsoldStock], window_traded: i64, span_secs: i64) -> (Vec<(UnsoldStock, f64)>, Vec<UnsoldStock>, Option<f64>) {
    let mut ranked: Vec<(UnsoldStock, f64)> = stock.iter().filter_map(|entry| entry.days(span_secs).map(|days| (entry.clone(), days))).collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.name.cmp(&right.0.name)));
    ranked.truncate(STOCK_TOP);
    let mut untraded: Vec<UnsoldStock> = stock.iter().filter(|entry| entry.traded_units <= 0 && entry.resting_units > 0).cloned().collect();
    untraded.sort_by(|left, right| right.resting_units.cmp(&left.resting_units).then_with(|| left.name.cmp(&right.name)));
    let resting: i64 = stock.iter().map(|entry| entry.resting_units).sum();
    (ranked, untraded, stock_days(resting, window_traded, span_secs))
}

/// Realized over predicted of the closed lots opened at or after
/// `since`: (lots, predicted, realized, ratio when predicted > 0).
pub fn lot_ratio(rows: &[TradeRow], since: i64) -> (usize, i64, i64, Option<f64>) {
    let inside: Vec<TradeRow> = rows.iter().filter(|row| row.opened >= since).cloned().collect();
    let (lots, predicted, realized) = crate::trade_report::closed_totals(&inside);
    (lots, predicted, realized, (predicted > 0).then(|| realized as f64 / predicted as f64))
}

/// Lots opened at or after `since` not closed yet (buying, crafting or
/// selling): their realized coins are still to come.
pub fn open_lots(rows: &[TradeRow], since: i64) -> usize {
    rows.iter().filter(|row| row.opened >= since && row.status != "closed").count()
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |ratio| format!("{:.1} %", ratio * 100.0))
}

fn lot_line(label: &str, (lots, predicted, realized, ratio): (usize, i64, i64, Option<f64>)) -> String {
    format!("{label}: {lots} closed lots, predicted {predicted:+}, realized {realized:+} = {}", percent(ratio))
}

/// The section of `orchestrator economy` over `[since, now)` (`hours`
/// long), staff (`excluded`) left out; `trade_rows` = the R5 lot rows.
pub fn print_section(store: &Store, trade_rows: &[TradeRow], since: i64, hours: i64, excluded: &[String]) {
    println!("équilibre (plan-equilibre, last {hours} h; baseline 2026-11-19 -> target §M):");
    let span = hours * 3600;
    match store.unsold_stock(since, 0, excluded).and_then(|stock| Ok((store.window_traded_units(since, 0)?, stock))) {
        Ok((traded, stock)) => {
            let resting: i64 = stock.iter().map(|entry| entry.resting_units).sum();
            let (ranked, untraded, fleet) = stock_ranking(&stock, traded, span);
            let fleet = fleet.map_or_else(|| "n/a".to_string(), |days| format!("{days:.2} days"));
            println!("  unsold stock (units in asks / units traded per day): fleet {fleet} ({resting} units resting, {traded} traded)   (no §M target: to follow)");
            for (entry, days) in &ranked {
                println!("    {:<28} {:>9.1} days {:>9} resting {:>8} traded", entry.name, days, entry.resting_units, entry.traded_units);
            }
            let untraded_units: i64 = untraded.iter().map(|entry| entry.resting_units).sum();
            let largest: Vec<String> = untraded.iter().take(5).map(|entry| format!("{} {}", entry.name, entry.resting_units)).collect();
            println!("    not traded in the window: {} objects, {untraded_units} units resting (largest: {})", untraded.len(), largest.join(", "));
        }
        Err(error) => println!("  unsold stock: {error}"),
    }
    match store.work_by_method(since, 0, excluded) {
        Ok(rows) => {
            let shares = work_shares(&rows);
            println!("  fleet work time: {} work ticks", shares.total);
            println!("    filler (fill-* nodes)          {:>8}   (20 % -> < 10 %)", percent(shares.share(shares.filler)));
            println!("    yew logs                       {:>8}   (21 % -> < 10 %)", percent(shares.share(shares.yew)));
            println!("    {:<30} {:>8}   (3.2 % -> > 10 %)", TRANSFORM_SKILLS.join("+"), percent(shares.share(shares.transform())));
            for (skill, ticks) in &shares.by_skill {
                println!("    {:<30} {:>8} {:>12} ticks", skill, percent(shares.share(*ticks)), ticks);
            }
        }
        Err(error) => println!("  fleet work time: {error}"),
    }
    println!("  100 % market lots realized / predicted   (16 % -> >= 70 %)");
    println!("    {}; {} lots still open", lot_line(&format!("opened in the last {hours} h"), lot_ratio(trade_rows, since)), open_lots(trade_rows, since));
    println!("    {}; {} lots still open", lot_line("all recorded", lot_ratio(trade_rows, i64::MIN)), open_lots(trade_rows, i64::MIN));
}

#[cfg(test)]
#[path = "economy_balance_tests.rs"]
mod tests;
