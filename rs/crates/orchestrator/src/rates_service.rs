#![forbid(unsafe_code)]
//! Yield rates (lot rendements): from one finished task engagement to a
//! published per-6000-ticks figure the planner can compare.
//!
//! Pipeline: the worker pushes a `TaskReport` JSON value (see
//! `session_report::report_value`); `jobs::ingest_report_into` stores the
//! raw rows in `lifescape-store` (`task_reports` + `task_yields`, never
//! edited here) and hands the parsed report to `Snapshot::record` below.
//! The snapshot is the aggregate state: one bucket per
//! (method, item, level-band) plus one per (method, skill, level-band)
//! for XP. It lives in memory, is rewritten atomically to
//! `run/rates-snapshot.json` on every ingest, and reloads from that file
//! when the daemon restarts, so no measured yield is lost with the
//! process. The store rows stay the durable raw log (re-cuttable later).
//!
//! Rules applied here, all measured or explicitly marked:
//! - Only farm flows count (D16): `produced`, `loot`, `ground_spawn` and
//!   `picked`. Shop and trade flows (`shop_bought`, `traded`, `shop_sold`),
//!   `consumed` inputs and `lost` never enter a yield rate; `consumed`
//!   lines feed the input-cost side of profitability instead.
//! - Normalisation per 6000 ticks of engagement, levels in bands of 5,
//!   per bucket: report count, mean, population standard deviation and
//!   an exponential moving average (alpha 0.3, arrival order).
//! - Cold start: `prior_for` estimates (gamedata tier tables for the XP
//!   side, era practice notes for the quantities) are served flagged
//!   `prior: true` while a bucket holds fewer than 3 measured reports,
//!   then replaced by the measured aggregate.
//! - Profitability of one (method, band): revenue at the estimated sale
//!   price (24 h market VWAP when the store has prints, else the shop
//!   price) minus input cost (consumed lines at shop price) minus the
//!   market-pause friction (paused ticks measured per report, priced at
//!   the activity's own revenue rate: paused time earns nothing) plus a
//!   trend bonus (public trend score times revenue times a small weight).
//! - shortcut: friction currently prices at the own-rate opportunity
//!   cost; a coin-denominated fee model lands when deal fees exist.
//! - shortcut: the pause column has no home in the v1 store schema, so
//!   pauses accumulate in the snapshot only; a store migration will
//!   persist them per report in a later lot.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use lifescape_json::{object, str_value, Value};
use lifescape_store::Store;

/// Ticks of one normalised engagement window (one in-game hour in prod).
pub const NORMALIZE_TICKS: f64 = 6000.0;
/// Width of a level band: bands are 1-5, 6-10, ...
pub const LEVEL_BAND: i64 = 5;
/// Measured reports from which a prior is dropped for the measurement.
pub const MIN_MEASURED_REPORTS: usize = 3;
/// Smoothing of the moving average, in arrival order.
pub const EMA_ALPHA: f64 = 0.3;
/// Weight of the public trend bonus inside profitability.
pub const TREND_WEIGHT: f64 = 0.1;
/// Snapshot schema, bumped when the file layout changes.
const SNAPSHOT_SCHEMA: i64 = 1;

/// Provenances that count as farm output. `picked` is accepted although
/// no session labels it yet (today such takes read `ground_spawn`).
pub fn is_farm_provenance(provenance: &str) -> bool {
    matches!(provenance, "produced" | "loot" | "ground_spawn" | "picked")
}

/// Provenances that count as an engagement input cost (bait, food).
pub fn is_input_provenance(provenance: &str) -> bool {
    matches!(provenance, "consumed")
}

/// First level of the band holding `level`: 1-5 -> 1, 6-10 -> 6.
/// Levels below 1 clamp to the first band.
pub fn level_band(level: i64) -> i64 {
    if level <= 1 {
        return 1;
    }
    ((level - 1) / LEVEL_BAND) * LEVEL_BAND + 1
}

/// Quantity of one report scaled to a full window. Non-positive tick
/// budgets carry no rate: the caller skips the line loudly.
pub fn normalize_per_6000(quantity: i64, work_ticks: i64) -> Option<f64> {
    if work_ticks <= 0 {
        return None;
    }
    Some(quantity as f64 * NORMALIZE_TICKS / work_ticks as f64)
}

/// Mean of the samples, 0.0 when empty.
pub fn mean(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

/// Population standard deviation, 0.0 below two samples.
pub fn stddev(samples: &[f64]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let average = mean(samples);
    let variance = samples.iter().map(|sample| (sample - average) * (sample - average)).sum::<f64>()
        / samples.len() as f64;
    variance.sqrt()
}

/// Exponential moving average in slice order, alpha 0.3: the first
/// sample seeds it. Empty slices read 0.0.
pub fn moving_average(samples: &[f64], alpha: f64) -> f64 {
    let mut ema = 0.0;
    let mut first = true;
    for sample in samples {
        if first {
            ema = *sample;
            first = false;
        } else {
            ema = alpha * sample + (1.0 - alpha) * ema;
        }
    }
    ema
}

/// Cold-start estimate for one (method, item) pair, per 6000 ticks with
/// its XP twin. Quantities come from era practice (which activity pays
/// at low level), XP from the gamedata tier tables (raw tenths per
/// product). Served flagged `prior` until 3 measured reports land.
pub fn prior_for(method: &str, item: &str) -> Option<(f64, f64)> {
    let key = (method.to_lowercase(), item.to_lowercase());
    match (key.0.as_str(), key.1.as_str()) {
        ("fishing", "raw shrimps") => Some((400.0, 4000.0)),
        ("fishing", "raw anchovies") => Some((120.0, 2400.0)),
        ("fishing", "raw sardine") => Some((150.0, 3000.0)),
        ("fishing", "raw trout") => Some((100.0, 5000.0)),
        ("fishing", "raw lobster") => Some((60.0, 5400.0)),
        ("woodcut", "logs") => Some((250.0, 3000.0)),
        ("woodcut", "oak logs") => Some((150.0, 4500.0)),
        ("woodcut", "willow logs") => Some((180.0, 5400.0)),
        ("mining", "copper ore") => Some((150.0, 2500.0)),
        ("mining", "tin ore") => Some((150.0, 2500.0)),
        ("mining", "iron ore") => Some((120.0, 4200.0)),
        ("mining", "coal") => Some((80.0, 4000.0)),
        ("melee", "bones") => Some((60.0, 2500.0)),
        _ => None,
    }
}

/// One parsed engagement, straight from the pushed report JSON. `ticks`
/// is the wall budget, `pause_ticks` the market-pause slice inside it;
/// rates divide by their difference (work ticks).
#[derive(Clone, Debug)]
pub struct ParsedReport {
    pub account: String,
    pub node_id: String,
    pub method: String,
    pub script: String,
    pub wall_ticks: i64,
    pub pause_ticks: i64,
    pub stop: String,
    pub level: i64,
    pub band: i64,
    pub items: Vec<ParsedItem>,
    pub xp: Vec<ParsedXp>,
}

/// One reported item flow.
#[derive(Clone, Debug)]
pub struct ParsedItem {
    pub name: String,
    pub provenance: String,
    pub quantity: i64,
}

/// One reported XP gain.
#[derive(Clone, Debug)]
pub struct ParsedXp {
    pub skill: String,
    pub amount: i64,
}

fn text_field(report: &Value, key: &str) -> String {
    report.get(key).as_str().unwrap_or("").to_string()
}

/// Main-skill level at engagement start: the level beside the first XP
/// skill when present, else the first band level seen, else 1.
fn context_level(report: &Value) -> i64 {
    let xp_skill_stat = report
        .get("xp")
        .as_arr()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("stat").as_i64());
    let bands = report.get("levels_before");
    let level_for = |stat: i64| {
        bands.as_arr().and_then(|rows| {
            rows.iter().find(|row| row.get("stat").as_i64() == Some(stat)).and_then(|row| row.get("level").as_i64())
        })
    };
    if let Some(stat) = xp_skill_stat {
        if let Some(level) = level_for(stat) {
            return level.max(1);
        }
    }
    if let Some(rows) = bands.as_arr() {
        if let Some(row) = rows.first() {
            return row.get("level").as_i64().unwrap_or(1).max(1);
        }
    }
    1
}

/// Parses the pushed report JSON into store + snapshot rows. `method`
/// falls back to `activity` (today's contract reports carry the activity
/// only; finer `gather:skill/resource` methods land with the planner).
/// Empty account or method is a caller bug, reported loudly.
pub fn parse_report(report: &Value) -> Result<ParsedReport, String> {
    let account = text_field(report, "account");
    if account.is_empty() {
        return Err("rates: report account is empty".to_string());
    }
    let activity = text_field(report, "activity");
    let explicit = text_field(report, "method");
    let method = if explicit.is_empty() { activity.clone() } else { explicit };
    if method.is_empty() {
        return Err("rates: report method/activity is empty".to_string());
    }
    let script = {
        let named = text_field(report, "script");
        if named.is_empty() { activity.clone() } else { named }
    };
    let wall_ticks = report.get("ticks").as_i64().unwrap_or(0);
    let pause_ticks = report.get("market_pause_ticks").as_i64().unwrap_or(0).max(0);
    let mut items = Vec::new();
    if let Some(rows) = report.get("items").as_arr() {
        for row in rows {
            let provenance = row.get("provenance").as_str().unwrap_or("").to_string();
            if provenance.is_empty() {
                return Err(format!("rates: item '{}' has no provenance", row.get("name").as_str().unwrap_or("?")));
            }
            items.push(ParsedItem {
                name: row.get("name").as_str().unwrap_or("").to_string(),
                provenance,
                quantity: row.get("qty").as_i64().unwrap_or(0),
            });
        }
    }
    let mut xp = Vec::new();
    if let Some(rows) = report.get("xp").as_arr() {
        for row in rows {
            let skill = row.get("skill").as_str().unwrap_or("").to_string();
            let amount = row.get("xp").as_i64().unwrap_or(0);
            if skill.is_empty() || amount == 0 {
                continue;
            }
            xp.push(ParsedXp { skill, amount });
        }
    }
    Ok(ParsedReport {
        account,
        node_id: text_field(report, "node_id"),
        method,
        script,
        wall_ticks,
        pause_ticks,
        stop: text_field(report, "stop"),
        level: context_level(report),
        band: level_band(context_level(report)),
        items,
        xp,
    })
}

/// Aggregate of one bucket: measured count plus mean, population
/// standard deviation and moving average, all per 6000 ticks. `prior`
/// marks a cold-start estimate (fewer than 3 reports with a known
/// prior); measured values replace it from 3 reports on.
#[derive(Clone, Debug)]
pub struct Aggregate {
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
    pub ema: f64,
    pub prior: bool,
}

/// Builds one aggregate from ordered samples with an optional prior.
pub fn aggregate(samples: &[f64], prior: Option<f64>) -> Aggregate {
    if samples.len() >= MIN_MEASURED_REPORTS {
        return Aggregate {
            count: samples.len(),
            mean: mean(samples),
            stddev: stddev(samples),
            ema: moving_average(samples, EMA_ALPHA),
            prior: false,
        };
    }
    if let Some(estimate) = prior {
        return Aggregate {
            count: samples.len(),
            mean: estimate,
            stddev: stddev(samples),
            ema: if samples.is_empty() { estimate } else { moving_average(samples, EMA_ALPHA) },
            prior: true,
        };
    }
    Aggregate {
        count: samples.len(),
        mean: mean(samples),
        stddev: stddev(samples),
        ema: moving_average(samples, EMA_ALPHA),
        prior: false,
    }
}

/// Profitability of one activity band, in coins per 6000 ticks:
/// revenue at estimated sale price, minus input cost, minus the pause
/// friction (paused ticks at the activity's own revenue rate), plus the
/// public-trend bonus (score times revenue times weight).
pub fn profit_per_6000(
    revenue_items: &[(f64, f64)],
    input_cost: f64,
    pause_ticks_per_6000: f64,
    trend_score: f64,
    trend_weight: f64,
) -> f64 {
    let revenue: f64 = revenue_items.iter().map(|(yield_per, price)| yield_per * price).sum();
    let revenue_rate = revenue / NORMALIZE_TICKS;
    let friction = pause_ticks_per_6000 * revenue_rate;
    revenue - input_cost - friction + trend_score * revenue * trend_weight
}

/// Running aggregate of one (method, item-or-skill, band) bucket.
#[derive(Clone, Debug, Default)]
struct BucketStat {
    count: usize,
    sum: f64,
    sum_squares: f64,
    ema: f64,
    ordered: Vec<f64>,
}

impl BucketStat {
    fn add(&mut self, sample: f64) {
        if self.count == 0 {
            self.ema = sample;
        } else {
            self.ema = EMA_ALPHA * sample + (1.0 - EMA_ALPHA) * self.ema;
        }
        self.count += 1;
        self.sum += sample;
        self.sum_squares += sample * sample;
        self.ordered.push(sample);
    }

    fn aggregate(&self, prior: Option<f64>) -> Aggregate {
        aggregate(&self.ordered, prior)
    }

    fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }
}

/// Whole rates state: farm yields, XP and input costs per bucket, plus
/// measured market pauses per (method, band) for the friction term.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    yields: BTreeMap<(String, String, i64), BucketStat>,
    xp: BTreeMap<(String, String, i64), BucketStat>,
    inputs: BTreeMap<(String, String, i64), BucketStat>,
    pauses: BTreeMap<(String, i64), BucketStat>,
}

impl Snapshot {
    pub fn new() -> Self {
        Snapshot::default()
    }

    /// Folds one parsed engagement into the buckets. Lines that carry
    /// no rate (zero tick budget, non-farm provenance) are skipped;
    /// the raw store rows keep them.
    pub fn record(&mut self, parsed: &ParsedReport) {
        let work_ticks = parsed.wall_ticks.saturating_sub(parsed.pause_ticks);
        if work_ticks <= 0 {
            return;
        }
        let band_key = (parsed.method.clone(), parsed.band);
        if let Some(pause_per) = normalize_per_6000(parsed.pause_ticks, work_ticks) {
            self.pauses.entry(band_key).or_default().add(pause_per);
        }
        for flow in &parsed.items {
            let Some(per_window) = normalize_per_6000(flow.quantity, work_ticks) else {
                continue;
            };
            if is_farm_provenance(&flow.provenance) {
                self.yields
                    .entry((parsed.method.clone(), flow.name.clone(), parsed.band))
                    .or_default()
                    .add(per_window);
            } else if is_input_provenance(&flow.provenance) {
                // Consumed quantities are negative flows: cost counts
                // the absolute depletion per window.
                self.inputs
                    .entry((parsed.method.clone(), flow.name.clone(), parsed.band))
                    .or_default()
                    .add(per_window.abs());
            }
        }
        for gain in &parsed.xp {
            if let Some(per_window) = normalize_per_6000(gain.amount, work_ticks) {
                self.xp
                    .entry((parsed.method.clone(), gain.skill.clone(), parsed.band))
                    .or_default()
                    .add(per_window);
            }
        }
    }

    /// Yield rows with their aggregates, priors applied below 3 reports.
    pub fn yield_rows(&self) -> Vec<YieldRow> {
        let mut rows = Vec::new();
        for ((method, item, band), stat) in &self.yields {
            let prior = prior_for(method, item).map(|(yield_per, _)| yield_per);
            let aggregate = stat.aggregate(prior);
            rows.push(YieldRow {
                method: method.clone(),
                item: item.clone(),
                band: *band,
                count: aggregate.count,
                mean: aggregate.mean,
                stddev: aggregate.stddev,
                ema: aggregate.ema,
                prior: aggregate.prior,
            });
        }
        rows.sort_by(|left, right| {
            (left.method.clone(), left.item.clone(), left.band).cmp(&(right.method.clone(), right.item.clone(), right.band))
        });
        rows
    }

    /// XP rows with their aggregates, same banding and priors.
    pub fn xp_rows(&self) -> Vec<XpRow> {
        let mut rows = Vec::new();
        for ((method, skill, band), stat) in &self.xp {
            let prior = self.xp_prior(method, skill);
            let aggregate = stat.aggregate(prior);
            rows.push(XpRow {
                method: method.clone(),
                skill: skill.clone(),
                band: *band,
                count: aggregate.count,
                mean: aggregate.mean,
                stddev: aggregate.stddev,
                ema: aggregate.ema,
                prior: aggregate.prior,
            });
        }
        rows.sort_by(|left, right| {
            (left.method.clone(), left.skill.clone(), left.band).cmp(&(right.method.clone(), right.skill.clone(), right.band))
        });
        rows
    }

    fn xp_prior(&self, method: &str, skill: &str) -> Option<f64> {
        // XP priors ride beside the item priors of the same method.
        xp_prior_for(method, skill)
    }

    /// Mean pause ticks per window for one (method, band).
    pub fn pause_per_6000(&self, method: &str, band: i64) -> f64 {
        self.pauses.get(&(method.to_string(), band)).map(|stat| stat.mean()).unwrap_or(0.0)
    }

    fn bucket_mean(&self, buckets: &BTreeMap<(String, String, i64), BucketStat>, method: &str, name: &str, band: i64) -> Option<f64> {
        buckets.get(&(method.to_string(), name.to_string(), band)).filter(|stat| stat.count > 0).map(|stat| stat.mean())
    }
}

/// Live snapshot clone for the brain (read-only planning input: public
/// yields only, never another bot's state).
pub fn live_snapshot() -> Snapshot {
    global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
}

/// Measured mean yield per 6000 for one (method, item, band) bucket.
pub fn measured_yield(snapshot: &Snapshot, method: &str, item: &str, band: i64) -> Option<f64> {
    snapshot.bucket_mean(&snapshot.yields, method, item, band)
}

/// Measured mean XP per 6000 for one (method, skill, band) bucket.
pub fn measured_xp(snapshot: &Snapshot, method: &str, skill: &str, band: i64) -> Option<f64> {
    snapshot.bucket_mean(&snapshot.xp, method, skill, band)
}

/// Cold-start XP estimate per (method, skill), twin of `prior_for`.
pub fn xp_prior_for(method: &str, skill: &str) -> Option<f64> {
    match (method.to_lowercase().as_str(), skill.to_lowercase().as_str()) {
        ("fishing", "fishing") => Some(4000.0),
        ("woodcut", "woodcutting") => Some(3500.0),
        ("mining", "mining") => Some(3000.0),
        ("melee", "attack") => Some(2000.0),
        ("melee", "strength") => Some(2000.0),
        ("melee", "defence") => Some(2000.0),
        ("melee", "hitpoints") => Some(1500.0),
        _ => None,
    }
}

/// One published yield row, per 6000 ticks.
#[derive(Clone, Debug)]
pub struct YieldRow {
    pub method: String,
    pub item: String,
    pub band: i64,
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
    pub ema: f64,
    pub prior: bool,
}

/// One published XP row, per 6000 ticks.
#[derive(Clone, Debug)]
pub struct XpRow {
    pub method: String,
    pub skill: String,
    pub band: i64,
    pub count: usize,
    pub mean: f64,
    pub stddev: f64,
    pub ema: f64,
    pub prior: bool,
}

fn yield_value(row: &YieldRow) -> Value {
    object(vec![
        ("method", str_value(&row.method)),
        ("item", str_value(&row.item)),
        ("band", Value::Int(row.band)),
        ("count", Value::Int(row.count as i64)),
        ("mean", Value::Float(row.mean)),
        ("stddev", Value::Float(row.stddev)),
        ("ema", Value::Float(row.ema)),
        ("prior", Value::Bool(row.prior)),
    ])
}

fn xp_value(row: &XpRow) -> Value {
    object(vec![
        ("method", str_value(&row.method)),
        ("skill", str_value(&row.skill)),
        ("band", Value::Int(row.band)),
        ("count", Value::Int(row.count as i64)),
        ("mean", Value::Float(row.mean)),
        ("stddev", Value::Float(row.stddev)),
        ("ema", Value::Float(row.ema)),
        ("prior", Value::Bool(row.prior)),
    ])
}

/// Estimated sale price of an item display name: 24 h market VWAP when
/// the store holds prints, else the shop price, else 0 with a prior
/// price flag. Name matching is exact on the catalogue display name.
pub fn price_estimate(store: &Store, item: &str) -> (f64, bool) {
    let catalogue = match store.list_items() {
        Ok(rows) => rows,
        Err(_) => return (0.0, true),
    };
    let Some(found) = catalogue.iter().find(|row| row.name == item) else {
        return (0.0, true);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_secs() as i64)
        .unwrap_or(0);
    let prints = store.executions_for_item(found.item_id, now - 86400, now + 1).unwrap_or_default();
    let volume: i64 = prints.iter().map(|print| print.quantity).sum();
    if volume > 0 {
        let turnover: i64 = prints.iter().map(|print| print.quantity * print.price).sum();
        return (turnover as f64 / volume as f64, false);
    }
    (found.shop_price as f64, true)
}

/// Public trend score of an item display name (24 h period preferred),
/// 0.0 when unscored.
pub fn trend_score(store: &Store, item: &str) -> f64 {
    let catalogue = match store.list_items() {
        Ok(rows) => rows,
        Err(_) => return 0.0,
    };
    let Some(found) = catalogue.iter().find(|row| row.name == item) else {
        return 0.0;
    };
    let scores = store.trends_for_item(found.item_id).unwrap_or_default();
    scores.iter().find(|trend| trend.period == "24h").map(|trend| trend.score).unwrap_or_else(|| {
        scores.first().map(|trend| trend.score).unwrap_or(0.0)
    })
}

/// One profitability row per (method, band): revenue at estimated
/// prices, input cost at shop prices, pause friction, trend bonus.
#[derive(Clone, Debug)]
pub struct ProfitRow {
    pub method: String,
    pub band: i64,
    pub revenue: f64,
    pub input_cost: f64,
    pub friction: f64,
    pub trend_bonus: f64,
    pub profit: f64,
    pub price_prior: bool,
}

/// Builds profitability from the snapshot, pricing through the store.
pub fn profit_rows(store: &Store, snapshot: &Snapshot) -> Vec<ProfitRow> {
    let mut bands: BTreeMap<(String, i64), Vec<(String, f64)>> = BTreeMap::new();
    for ((method, item, band), stat) in &snapshot.yields {
        bands.entry((method.clone(), *band)).or_default().push((item.clone(), stat.mean()));
    }
    let mut rows = Vec::new();
    for ((method, band), products) in bands {
        let mut revenue_items = Vec::new();
        let mut price_prior = false;
        let mut trend_weighted = 0.0;
        for (item, mean) in &products {
            let (price, prior) = price_estimate(store, item);
            price_prior = price_prior || prior;
            revenue_items.push((*mean, price));
            trend_weighted += mean * price * trend_score(store, item);
        }
        let revenue: f64 = revenue_items.iter().map(|(yield_per, price)| yield_per * price).sum();
        let mut input_cost = 0.0;
        for ((input_method, input_item, input_band), stat) in &snapshot.inputs {
            if input_method == &method && input_band == &band {
                let (price, prior) = price_estimate(store, input_item);
                price_prior = price_prior || prior;
                input_cost += stat.mean() * price;
            }
        }
        let pause_per = snapshot.pause_per_6000(&method, band);
        let score = if revenue > 0.0 { trend_weighted / revenue } else { 0.0 };
        let profit = profit_per_6000(&revenue_items, input_cost, pause_per, score, TREND_WEIGHT);
        let friction = pause_per * revenue / NORMALIZE_TICKS;
        let trend_bonus = score * revenue * TREND_WEIGHT;
        rows.push(ProfitRow {
            method: method.clone(),
            band,
            revenue,
            input_cost,
            friction,
            trend_bonus,
            profit,
            price_prior,
        });
    }
    rows.sort_by(|left, right| (left.method.clone(), left.band).cmp(&(right.method.clone(), right.band)));
    rows
}

fn profit_value(row: &ProfitRow) -> Value {
    object(vec![
        ("method", str_value(&row.method)),
        ("band", Value::Int(row.band)),
        ("revenue", Value::Float(row.revenue)),
        ("input_cost", Value::Float(row.input_cost)),
        ("friction", Value::Float(row.friction)),
        ("trend_bonus", Value::Float(row.trend_bonus)),
        ("profit", Value::Float(row.profit)),
        ("price_prior", Value::Bool(row.price_prior)),
    ])
}

/// Snapshot file beside the staging log: rewritten on every ingest,
/// reloaded when the daemon restarts.
pub fn snapshot_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LIFESCAPE_RATES_PATH") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    crate::paths::run_dir().join("rates-snapshot.json")
}

fn global_snapshot() -> &'static Mutex<Snapshot> {
    static SNAPSHOT: OnceLock<Mutex<Snapshot>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| Mutex::new(load_snapshot_file()))
}

fn load_snapshot_file() -> Snapshot {
    let path = snapshot_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Snapshot::new(),
    };
    snapshot_from_json(&text).unwrap_or_default()
}

fn snapshot_to_value(snapshot: &Snapshot) -> Value {
    let mut yields = BTreeMap::new();
    for ((method, item, band), stat) in &snapshot.yields {
        yields.insert(
            format!("{method}\u{1f}{item}\u{1f}{band}"),
            object(vec![
                ("count", Value::Int(stat.count as i64)),
                ("sum", Value::Float(stat.sum)),
                ("sum_squares", Value::Float(stat.sum_squares)),
                ("ema", Value::Float(stat.ema)),
                ("ordered", Value::Arr(stat.ordered.iter().map(|sample| Value::Float(*sample)).collect())),
            ]),
        );
    }
    let mut xp = BTreeMap::new();
    for ((method, skill, band), stat) in &snapshot.xp {
        xp.insert(
            format!("{method}\u{1f}{skill}\u{1f}{band}"),
            object(vec![
                ("count", Value::Int(stat.count as i64)),
                ("sum", Value::Float(stat.sum)),
                ("sum_squares", Value::Float(stat.sum_squares)),
                ("ema", Value::Float(stat.ema)),
                ("ordered", Value::Arr(stat.ordered.iter().map(|sample| Value::Float(*sample)).collect())),
            ]),
        );
    }
    let mut inputs = BTreeMap::new();
    for ((method, item, band), stat) in &snapshot.inputs {
        inputs.insert(
            format!("{method}\u{1f}{item}\u{1f}{band}"),
            object(vec![
                ("count", Value::Int(stat.count as i64)),
                ("sum", Value::Float(stat.sum)),
                ("sum_squares", Value::Float(stat.sum_squares)),
                ("ema", Value::Float(stat.ema)),
                ("ordered", Value::Arr(stat.ordered.iter().map(|sample| Value::Float(*sample)).collect())),
            ]),
        );
    }
    let mut pauses = BTreeMap::new();
    for ((method, band), stat) in &snapshot.pauses {
        pauses.insert(
            format!("{method}\u{1f}{band}"),
            object(vec![
                ("count", Value::Int(stat.count as i64)),
                ("sum", Value::Float(stat.sum)),
                ("sum_squares", Value::Float(stat.sum_squares)),
                ("ema", Value::Float(stat.ema)),
                ("ordered", Value::Arr(stat.ordered.iter().map(|sample| Value::Float(*sample)).collect())),
            ]),
        );
    }
    object(vec![
        ("schema", Value::Int(SNAPSHOT_SCHEMA)),
        ("yields", Value::Obj(yields)),
        ("xp", Value::Obj(xp)),
        ("inputs", Value::Obj(inputs)),
        ("pauses", Value::Obj(pauses)),
    ])
}

fn read_bucket(value: &Value) -> BucketStat {
    let floats = |key: &str| {
        value.get(key).as_arr().map(|rows| rows.iter().filter_map(|row| row.as_f64()).collect::<Vec<f64>>()).unwrap_or_default()
    };
    let ordered = floats("ordered");
    let (count, sum, sum_squares, ema) = if ordered.is_empty() {
        (
            value.get("count").as_i64().unwrap_or(0).max(0) as usize,
            value.get("sum").as_f64().unwrap_or(0.0),
            value.get("sum_squares").as_f64().unwrap_or(0.0),
            value.get("ema").as_f64().unwrap_or(0.0),
        )
    } else {
        let sum: f64 = ordered.iter().sum();
        let sum_squares: f64 = ordered.iter().map(|sample| sample * sample).sum();
        (ordered.len(), sum, sum_squares, moving_average(&ordered, EMA_ALPHA))
    };
    BucketStat { count, sum, sum_squares, ema, ordered }
}

fn split_three(key: &str) -> Option<(String, String, i64)> {
    let mut parts = key.split('\u{1f}');
    let first = parts.next()?.to_string();
    let second = parts.next()?.to_string();
    let third: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((first, second, third))
}

fn snapshot_from_json(text: &str) -> Option<Snapshot> {
    let value = lifescape_json::parse(text).ok()?;
    if value.get("schema").as_i64() != Some(SNAPSHOT_SCHEMA) {
        return None;
    }
    let mut snapshot = Snapshot::new();
    if let Some(map) = value.get("yields").as_obj() {
        for (key, entry) in map {
            if let Some((method, item, band)) = split_three(key) {
                snapshot.yields.insert((method, item, band), read_bucket(entry));
            }
        }
    }
    if let Some(map) = value.get("xp").as_obj() {
        for (key, entry) in map {
            if let Some((method, skill, band)) = split_three(key) {
                snapshot.xp.insert((method, skill, band), read_bucket(entry));
            }
        }
    }
    if let Some(map) = value.get("inputs").as_obj() {
        for (key, entry) in map {
            if let Some((method, item, band)) = split_three(key) {
                snapshot.inputs.insert((method, item, band), read_bucket(entry));
            }
        }
    }
    if let Some(map) = value.get("pauses").as_obj() {
        for (key, entry) in map {
            let mut parts = key.split('\u{1f}');
            let (Some(method), Some(band)) = (parts.next(), parts.next()) else {
                continue;
            };
            if parts.next().is_some() {
                continue;
            }
            if let Ok(band) = band.parse::<i64>() {
                snapshot.pauses.insert((method.to_string(), band), read_bucket(entry));
            }
        }
    }
    Some(snapshot)
}

/// Records one parsed report in the live snapshot and persists the file.
/// Loud error, never a silent drop: the store rows are already written
/// by the caller, so a snapshot failure must shout, not vanish.
pub fn record_snapshot(parsed: &ParsedReport) -> Result<(), String> {
    {
        let mut guard = global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.record(parsed);
        let value = snapshot_to_value(&guard);
        drop(guard);
        let path = snapshot_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("rates snapshot dir: {error}"))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, lifescape_json::stringify(&value))
            .map_err(|error| format!("rates snapshot write: {error}"))?;
        std::fs::rename(&tmp, &path).map_err(|error| format!("rates snapshot rename: {error}"))?;
    }
    Ok(())
}

/// Dashboard payload: opens the daemon store, serves yield rows, XP
/// rows and profitability, plus engaged counts per script counted from
/// the status list. Counts are operator eyes only (this page and API),
/// never read by bots.
pub fn rates_api(call: &dyn Fn(Value) -> Value) -> Value {
    let store = match Store::open(&crate::paths::run_dir().join("lifescape.db")) {
        Ok(store) => store,
        Err(error) => {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("rates store: {error}"))),
            ])
        }
    };
    let mut engaged: BTreeMap<String, i64> = BTreeMap::new();
    let status = call(object(vec![("cmd", str_value("status"))]));
    for job in status.get("jobs").as_arr().cloned().unwrap_or_default() {
        if job.get("phase").as_str() != Some("running") {
            continue;
        }
        let script = job.get("script").as_str().unwrap_or("").to_string();
        if script.is_empty() {
            continue;
        }
        *engaged.entry(script).or_insert(0) += 1;
    }
    let guard = global_snapshot().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let payload = rates_payload(&store, &guard, engaged);
    drop(guard);
    payload
}

/// Full rates payload for the dashboard: yield rows, XP rows,
/// profitability (priced through the store) and engaged counts supplied
/// by the caller (operator eyes only, never read by bots).
pub fn rates_payload(store: &Store, snapshot: &Snapshot, engaged: BTreeMap<String, i64>) -> Value {
    let yields: Vec<Value> = snapshot.yield_rows().iter().map(yield_value).collect();
    let xp: Vec<Value> = snapshot.xp_rows().iter().map(xp_value).collect();
    let profits: Vec<Value> = profit_rows(store, snapshot).iter().map(profit_value).collect();
    let mut engaged_map = BTreeMap::new();
    for (script, count) in engaged {
        engaged_map.insert(script, Value::Int(count));
    }
    object(vec![
        ("ok", Value::Bool(true)),
        ("rates", Value::Arr(yields)),
        ("xp", Value::Arr(xp)),
        ("profit", Value::Arr(profits)),
        ("engaged", Value::Obj(engaged_map)),
    ])
}

#[cfg(test)]
#[path = "rates_service_tests.rs"]
mod rates_tests;
