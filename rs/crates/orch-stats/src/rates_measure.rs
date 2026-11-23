#![forbid(unsafe_code)]
//! Admission of one contract-2 report into the yield estimator (plan
//! rendements R2): its D2 key, its sample, the checks that keep it out.
//!
//! - the key names the ACTIVITY (`planner::activity`): a drop farm is
//!   the fight it runs (`farm:feather` with `melee:attack` on chickens =
//!   `fight:melee/chickens`), a fishing leg its spot option;
//! - a report without a planner method (`method_source = none`), a
//!   trading or tutorial run, or a method naming no game way is never
//!   aggregated (logged, nothing guessed);
//! - only the yield causes (`RATED_CAUSES`) enter a sample: market
//!   trades, shop purchases and sales, and unattributed flows stay in the
//!   raw store, never in a rate;
//! - `failed` / `blocked` runs count as failures of their key only;
//! - a run carrying more units than the server allows (the physical
//!   ceiling of `method_rate::units_ceiling_per_tick`, plus one action
//!   of slack for the run's edges) is rejected and logged;
//! - D4: the XP derived from the run's units (`unit_xp`) is compared to
//!   the XP the run observed; a gap above `XP_GAP_ALERT` is logged, never
//!   a rate (the observed XP is a check, not a measure).

use lifescape_planner::method_rate::{units_ceiling_per_tick, PriorError, PriorQuery};

use crate::rates_estimator::{RunSample, YieldKey};
use crate::report_contract::ParsedContract;

/// Relative gap between derived and observed XP that raises an alert.
pub const XP_GAP_ALERT: f64 = 0.05;
/// Actions of slack above the ceiling (a roll landing on the run's edge).
pub const CEILING_SLACK_ACTIONS: f64 = 1.0;
/// Fastest F2P attack interval (darts on rapid): a kill needs a hit.
pub const FASTEST_ATTACK_TICKS: f64 = 2.0;
/// Flow causes that are a yield of the activity (§8 signed balance):
/// what it makes, eats, burns, picks up, pays and loses. Everything else
/// (`traded`, `shop_bought`, `shop_sold`: transfers D16/D44;
/// `unattributed`: an alert) stays in the store, never in a rate.
pub const RATED_CAUSES: [&str; 13] = ["product", "byproduct", "input", "ammo", "rune", "bait", "supply", "food", "discarded", "loot", "ground_spawn", "fee", "death"];

/// True for a flow cause that enters a rate.
pub fn rated_cause(cause: &str) -> bool {
    RATED_CAUSES.contains(&cause)
}

/// Where one report goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Rated(YieldKey, RunSample),
    Failed(YieldKey, i64),
    Rejected(YieldKey, String),
    Skipped(String),
}

/// Skill whose band keys a method (`planner::activity::band_skill`).
pub fn band_skill(method: &str, style: &str) -> Option<String> {
    lifescape_planner::activity::band_skill(method, style)
}

/// Band of a key: the band of `level` in the band skill, 0 without one.
pub fn band_for(method: &str, style: &str, level: i64) -> i64 {
    match band_skill(method, style) {
        Some(_) => crate::rates_service::level_band(level),
        None => 0,
    }
}

/// A method the estimator rates: a production, gather, spell or fight
/// (every family `method_rate` knows a way for).
pub fn rated_method(method: &str) -> bool {
    if method.is_empty() || crate::rates_service::is_trading_method(method) || crate::rates_service::is_unrated_method(method) {
        return false;
    }
    let query = PriorQuery { method: method.to_string(), level: 99, ..PriorQuery::default() };
    !matches!(lifescape_planner::method_rate::action_of(&query), Err(PriorError::NoWay(_)))
}

fn level_of(contract: &ParsedContract, skill: &str) -> Option<i64> {
    contract.levels.iter().find(|(name, _)| name == skill).map(|(_, level)| *level)
}

/// Key of a contract-2 report (`None` with the reason when it has none):
/// the activity the run worked (`planner::activity`), its style, site,
/// tool and band.
pub fn key_of(contract: &ParsedContract) -> Result<YieldKey, String> {
    if contract.planner_method.is_empty() {
        return Err(format!("no planner method (source {})", if contract.method_source.is_empty() { "none" } else { &contract.method_source }));
    }
    let activity = lifescape_planner::activity::activity_of_run(&contract.planner_method, &contract.combat_style, &contract.target, &contract.units);
    let method = activity.as_str();
    if !rated_method(method) {
        return Err(format!("{method} is no rated method"));
    }
    let band = match band_skill(method, &contract.combat_style) {
        Some(skill) => crate::rates_service::level_band(level_of(contract, &skill).ok_or_else(|| format!("{method}: no start level of {skill}"))?),
        None => 0,
    };
    Ok(YieldKey { method: method.to_string(), style: contract.combat_style.clone(), site: contract.site.clone(), tool: contract.tool.clone(), band })
}

fn site_of(text: &str) -> Option<(i64, i64, i64)> {
    let parts: Vec<i64> = text.split(',').filter_map(|part| part.trim().parse().ok()).collect();
    (parts.len() == 3).then(|| (parts[0], parts[1], parts[2]))
}

/// Units of each kind the server allows in `work_ticks` for the key (a
/// `PriorError` when no timing is modelled: accepted unchecked, logged).
pub fn ceiling(key: &YieldKey, level: i64, work_ticks: i64) -> Result<Vec<(String, f64)>, PriorError> {
    if key.method.starts_with("fight:") || !key.style.is_empty() {
        return Ok(vec![("kill".to_string(), work_ticks as f64 / FASTEST_ATTACK_TICKS + CEILING_SLACK_ACTIONS)]);
    }
    let query = PriorQuery { method: key.method.clone(), level: level.clamp(1, 99) as u8, tool: key.tool.clone(), site: site_of(&key.site) };
    let per_tick = units_ceiling_per_tick(&query)?;
    let ticks = lifescape_planner::method_rate::action_of(&query)?.ticks.max(1e-9);
    // Attempts the work holds, plus one of slack, each at its units.
    let attempts = work_ticks as f64 / ticks + CEILING_SLACK_ACTIONS;
    let mut out: Vec<(String, f64)> = Vec::new();
    for (kind, _, per) in per_tick {
        let units = per * ticks * attempts;
        match out.iter_mut().find(|(known, _)| known == kind) {
            Some(row) => row.1 += units,
            None => out.push((kind.to_string(), units)),
        }
    }
    Ok(out)
}

/// The first unit kind above its ceiling, as a reason.
fn above_ceiling(key: &YieldKey, contract: &ParsedContract, level: i64) -> Option<String> {
    let limits = match ceiling(key, level, contract.work_ticks) {
        Ok(limits) => limits,
        Err(error) => {
            crate::logln!("rates: no ceiling for {} ({error}), sample accepted unchecked", key.method);
            return None;
        }
    };
    for (kind, limit) in limits {
        let seen: i64 = contract.units.iter().filter(|(unit, _, _)| *unit == kind).map(|(_, _, qty)| *qty).sum();
        if seen as f64 > limit {
            return Some(format!("{seen} {kind} in {} work ticks, ceiling {limit:.1}", contract.work_ticks));
        }
    }
    None
}

/// Admission of one report (`stop` and the clock from the push).
pub fn admit(contract: &ParsedContract, stop: &str, clock: i64) -> Admission {
    let key = match key_of(contract) {
        Ok(key) => key,
        Err(reason) => return Admission::Skipped(reason),
    };
    if matches!(stop, "failed" | "blocked") {
        return Admission::Failed(key, contract.work_ticks);
    }
    if contract.work_ticks <= 0 {
        return Admission::Skipped(format!("{}: no work ticks", key.method));
    }
    let level = band_skill(&key.method, &key.style).and_then(|skill| level_of(contract, &skill)).unwrap_or(1);
    if let Some(reason) = above_ceiling(&key, contract, level) {
        return Admission::Rejected(key, reason);
    }
    let flows = contract.flows.iter().filter(|(_, cause, _)| rated_cause(cause)).cloned().collect();
    let sample = RunSample { work_ticks: contract.work_ticks, pause_ticks: contract.pause_ticks, flows, units: contract.units.clone(), clock };
    Admission::Rated(key, sample)
}

/// One skill whose derived and observed XP disagree beyond the alert.
#[derive(Debug, Clone, PartialEq)]
pub struct XpGap {
    pub skill: String,
    pub derived: f64,
    pub observed: f64,
}

/// D4 check: XP derived from the run's units against the observed XP
/// (`observed`: skill, XP gained). Gaps above `XP_GAP_ALERT`.
pub fn xp_gaps(contract: &ParsedContract, observed: &[(String, i64)]) -> Vec<XpGap> {
    let activity = lifescape_planner::activity::activity_of_run(&contract.planner_method, &contract.combat_style, &contract.target, &contract.units);
    let method = activity.as_str();
    let magic = level_of(contract, "magic").unwrap_or(1).clamp(1, 99) as u8;
    let units: Vec<(String, String, f64)> = contract.units.iter().map(|(kind, subject, qty)| (kind.clone(), subject.clone(), *qty as f64)).collect();
    let derived = lifescape_planner::unit_xp::derived_xp(method, &contract.combat_style, &crate::rates_names::units_in_debug(&units), magic);
    let mut skills: Vec<String> = derived.iter().map(|(skill, _)| skill.clone()).chain(observed.iter().map(|(skill, _)| skill.clone())).collect();
    skills.sort();
    skills.dedup();
    let mut gaps = Vec::new();
    for skill in skills {
        let made = derived.iter().find(|(name, _)| *name == skill).map(|(_, xp)| *xp).unwrap_or(0.0);
        let seen = observed.iter().filter(|(name, _)| *name == skill).map(|(_, xp)| *xp as f64).sum::<f64>();
        let scale = made.abs().max(seen.abs());
        if scale > 0.0 && (made - seen).abs() / scale > XP_GAP_ALERT {
            gaps.push(XpGap { skill, derived: made, observed: seen });
        }
    }
    gaps
}

#[cfg(test)]
#[path = "rates_measure_tests.rs"]
mod tests;
