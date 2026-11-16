#![forbid(unsafe_code)]
//! Reading a yield (plan rendements R2, R3.1): one estimate per D2 key
//! for the page AND the brain, same rules.
//!
//! - the buckets are keyed by ACTIVITY (`planner::activity`): a query
//!   names a planner method (`farm:raw_salmon`), the read looks its
//!   activity up (`gather:fishing/raw_trout`); the prior stays the
//!   queried method's own (a drop farm's prior yields its object);
//! - key fallback, explicit and logged: the exact key (site known), else
//!   the method on every site, else on every site and tool tier; a level
//!   only counts with `MIN_MEASURED_REPORTS` runs or more;
//! - below that threshold the game prior serves alone (`method_rate`,
//!   flagged `prior`); from it, measure and prior blend: the prior weighs
//!   `PRIOR_WEIGHT_TICKS` of work beside the bucket's decayed work ticks;
//! - no measure and no computable prior: a typed `RateError`, never a
//!   default figure;
//! - the XP per 6000 is derived from the unit rates (`unit_xp`), never
//!   measured.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use lifescape_planner::method_rate::{method_prior, PriorError, PriorQuery, RatePrior};

use crate::rates_estimator::{Bucket, YieldBook, YieldKey};
use crate::rates_measure::band_for;
use crate::rates_service::MIN_MEASURED_REPORTS;

/// Work ticks the prior weighs in a blend (one window).
pub const PRIOR_WEIGHT_TICKS: f64 = 6000.0;
/// Priors memoised at most (cleared beyond).
pub const PRIOR_CACHE_MAX: usize = 4096;

/// What a reader asks: a method run by a bot at its level, with its tool
/// tier and site when known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateQuery {
    pub method: String,
    pub style: String,
    pub site: Option<(i64, i64, i64)>,
    pub tool: String,
    pub level: u8,
}

/// Which key served the measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    Exact,
    AllSites,
    AllSitesTools,
    /// No key held enough runs: the prior alone.
    PriorOnly,
}

/// Confidence of an estimate (D9): runs, age, dispersion, failures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Confidence {
    pub runs: u64,
    pub age_secs: i64,
    pub dispersion: f64,
    pub failure_rate: f64,
    pub rejected: u64,
}

/// One published estimate, per 6000 ticks of work.
#[derive(Debug, Clone, PartialEq)]
pub struct RateEstimate {
    pub key: YieldKey,
    pub fallback: Fallback,
    /// Served from the prior alone.
    pub prior: bool,
    /// (display item, cause, signed per 6000).
    pub flows: Vec<(String, String, f64)>,
    /// (unit kind, subject, per 6000).
    pub units: Vec<(String, String, f64)>,
    /// (skill, XP per 6000) derived from the units.
    pub xp: Vec<(String, f64)>,
    pub pause_per_6000: f64,
    pub confidence: Confidence,
}

impl RateEstimate {
    /// Positive per-6000 rate of one display item (products, loot,
    /// pickups), 0 when it never comes in.
    pub fn gained(&self, display: &str) -> f64 {
        self.flows.iter().filter(|(item, _, qty)| item.eq_ignore_ascii_case(display) && *qty > 0.0).map(|(_, _, qty)| qty).sum()
    }
}

/// Why no estimate exists.
#[derive(Debug, Clone, PartialEq)]
pub enum RateError {
    /// Fewer than `MIN_MEASURED_REPORTS` runs and the prior failed.
    NoPrior { method: String, cause: PriorError },
}

impl std::fmt::Display for RateError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateError::NoPrior { method, cause } => write!(out, "no rate for {method}: unmeasured and {cause}"),
        }
    }
}

impl std::error::Error for RateError {}

fn site_text(site: Option<(i64, i64, i64)>) -> String {
    site.map(|(x, z, level)| format!("{x},{z},{level}")).unwrap_or_default()
}

/// Key of a query: the activity of its method (its band from the level
/// in the activity's skill).
pub fn key_of_query(query: &RateQuery) -> YieldKey {
    let activity = lifescape_planner::activity::activity_of_method(&query.method);
    let band = band_for(&activity, &query.style, i64::from(query.level));
    YieldKey { method: activity, style: query.style.clone(), site: site_text(query.site), tool: query.tool.clone(), band }
}

type PriorKey = (bool, String, u8, String, Option<(i64, i64, i64)>);

fn memo_prior(guide: bool, query: &PriorQuery) -> Result<RatePrior, PriorError> {
    static CACHE: OnceLock<Mutex<HashMap<PriorKey, Result<RatePrior, PriorError>>>> = OnceLock::new();
    let key = (guide, query.method.clone(), query.level, query.tool.clone(), query.site);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&key) {
        return found.clone();
    }
    let computed = if guide { lifescape_planner::method_rate::guide_prior(query) } else { method_prior(query) };
    let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() >= PRIOR_CACHE_MAX {
        guard.clear();
    }
    guard.insert(key, computed.clone());
    computed
}

/// The game prior of a key, memoised (the dump lookups are slow).
pub fn cached_prior(query: &PriorQuery) -> Result<RatePrior, PriorError> {
    memo_prior(false, query)
}

/// The guides' prior (R4, `method_rate::guide_prior`: a fight's drop
/// table priced in), memoised apart from the brain's.
pub fn cached_guide_prior(query: &PriorQuery) -> Result<RatePrior, PriorError> {
    memo_prior(true, query)
}

/// Logs a fallback once per (key, level).
fn log_fallback(key: &YieldKey, level: Fallback, reason: &str) {
    static SEEN: OnceLock<Mutex<HashSet<(YieldKey, String)>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let mut guard = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() > PRIOR_CACHE_MAX {
        guard.clear();
    }
    if guard.insert((key.clone(), format!("{level:?}"))) {
        crate::logln!("rates: {} {} site '{}' tool '{}' band {}: read {level:?} ({reason})", key.method, key.style, key.site, key.tool, key.band);
    }
}

/// Prior flows and units in report names (display items).
fn prior_rows(prior: &RatePrior) -> (Vec<(String, String, f64)>, Vec<(String, String, f64)>) {
    let flows = prior.flows.iter().map(|(item, cause, qty)| (crate::rates_names::display_of(item), cause.to_string(), *qty)).collect();
    let units = prior
        .units
        .iter()
        .map(|(kind, subject, qty)| {
            let subject = if matches!(*kind, "product" | "input") { crate::rates_names::display_of(subject) } else { subject.clone() };
            (kind.to_string(), subject, *qty)
        })
        .collect();
    (flows, units)
}

/// (measured × T + prior × P) / (T + P) per name; rows of either side.
fn blend(measured: &[(String, String, f64)], work_ticks: f64, prior: &[(String, String, f64)]) -> Vec<(String, String, f64)> {
    let mut rows: BTreeMap<(String, String), (f64, f64)> = BTreeMap::new();
    for (first, second, rate) in measured {
        rows.entry((first.clone(), second.clone())).or_default().0 += rate;
    }
    for (first, second, rate) in prior {
        rows.entry((first.clone(), second.clone())).or_default().1 += rate;
    }
    let total = work_ticks + PRIOR_WEIGHT_TICKS;
    rows.into_iter().map(|((first, second), (seen, guessed))| (first, second, (seen * work_ticks + guessed * PRIOR_WEIGHT_TICKS) / total)).collect()
}

fn confidence(bucket: Option<&Bucket>, now: i64) -> Confidence {
    let Some(bucket) = bucket else { return Confidence::default() };
    Confidence {
        runs: bucket.runs,
        age_secs: if bucket.last_clock > 0 { (now - bucket.last_clock).max(0) } else { 0 },
        dispersion: bucket.dispersion(),
        failure_rate: bucket.failure_rate(),
        rejected: bucket.rejected,
    }
}

fn derived(query: &RateQuery, units: &[(String, String, f64)]) -> Vec<(String, f64)> {
    lifescape_planner::unit_xp::derived_xp(&query.method, &query.style, &crate::rates_names::units_in_debug(units), query.level.max(1))
}

/// The estimate of `query` from `book` at wall clock `now` (the brain's
/// prior, `cached_prior`).
pub fn estimate(book: &YieldBook, query: &RateQuery, now: i64) -> Result<RateEstimate, RateError> {
    estimate_using(book, query, now, &cached_prior)
}

/// The estimate of `query` with the prior `prior_of` (the guides pass
/// `cached_guide_prior`).
pub fn estimate_using(book: &YieldBook, query: &RateQuery, now: i64, prior_of: &dyn Fn(&PriorQuery) -> Result<RatePrior, PriorError>) -> Result<RateEstimate, RateError> {
    let exact = key_of_query(query);
    let mut ladder: Vec<(YieldKey, Fallback)> = Vec::new();
    if query.site.is_some() {
        ladder.push((exact.clone(), Fallback::Exact));
    }
    ladder.push((exact.any_site(), Fallback::AllSites));
    ladder.push((exact.any_site_tool(), Fallback::AllSitesTools));
    let prior_query = PriorQuery { method: query.method.clone(), level: query.level.max(1), tool: query.tool.clone(), site: query.site };
    let prior = prior_of(&prior_query);
    let measured = ladder.iter().find(|(key, _)| book.get(key).is_some_and(|bucket| bucket.runs as usize >= MIN_MEASURED_REPORTS));
    let thinnest = ladder.iter().find_map(|(key, _)| book.get(key));
    let Some((key, fallback)) = measured else {
        let prior = prior.map_err(|cause| RateError::NoPrior { method: query.method.clone(), cause })?;
        let (flows, units) = prior_rows(&prior);
        let xp = derived(query, &units);
        return Ok(RateEstimate { key: exact, fallback: Fallback::PriorOnly, prior: true, flows, units, xp, pause_per_6000: 0.0, confidence: confidence(thinnest, now) });
    };
    if *fallback != Fallback::Exact {
        let reason = if query.site.is_none() { "no site known" } else { "exact key under the threshold" };
        log_fallback(&exact, *fallback, reason);
    }
    let bucket = book.get(key).cloned().unwrap_or_default();
    let (flows, units) = match &prior {
        Ok(prior) => {
            let (prior_flows, prior_units) = prior_rows(prior);
            (blend(&bucket.flow_rates(), bucket.work_ticks, &prior_flows), blend(&bucket.unit_rates(), bucket.work_ticks, &prior_units))
        }
        Err(_) => (bucket.flow_rates(), bucket.unit_rates()),
    };
    let xp = derived(query, &units);
    Ok(RateEstimate { key: key.clone(), fallback: *fallback, prior: false, flows, units, xp, pause_per_6000: bucket.pause_per_6000(), confidence: confidence(Some(&bucket), now) })
}

/// Skill-gate level of a method's way (1 without a gate): the level a
/// reader that knows no bot reads at.
pub fn gate_level(method: &str) -> u8 {
    lifescape_planner::method_way::method_way(method)
        .or_else(|| lifescape_planner::spell_methods::spell_way(method))
        .and_then(|(_, way)| way.skill.map(|(_, level)| level))
        .unwrap_or(1)
        .clamp(1, 99) as u8
}

/// The brain's read: `display` gained per 6000 ticks of `method` for a
/// bot at `level` with `tool` (no site: every site of the key).
pub fn gained_per_6000(book: &YieldBook, method: &str, display: &str, level: u8, tool: &str) -> Result<f64, RateError> {
    let query = RateQuery { method: method.to_string(), style: String::new(), site: None, tool: tool.to_string(), level };
    estimate(book, &query, crate::rates_service::now_secs()).map(|read| read.gained(display))
}

/// Logs a rate error once per method (a reader asks every pass).
pub fn log_rate_error(error: &RateError) {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let mut guard = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() > PRIOR_CACHE_MAX {
        guard.clear();
    }
    if guard.insert(error.to_string()) {
        crate::logln!("rates: {error}");
    }
}

/// Estimate of an existing bucket key, as the page reads it (its band's
/// first level, its own site and tool).
pub fn estimate_key(book: &YieldBook, key: &YieldKey, now: i64) -> Result<RateEstimate, RateError> {
    let site = {
        let parts: Vec<i64> = key.site.split(',').filter_map(|part| part.trim().parse().ok()).collect();
        (parts.len() == 3).then(|| (parts[0], parts[1], parts[2]))
    };
    let tool = if key.tool == crate::rates_estimator::ANY { String::new() } else { key.tool.clone() };
    let query = RateQuery { method: key.method.clone(), style: key.style.clone(), site, tool, level: key.band.clamp(1, 99) as u8 };
    estimate(book, &query, now)
}

#[cfg(test)]
#[path = "rates_read_tests.rs"]
mod tests;
