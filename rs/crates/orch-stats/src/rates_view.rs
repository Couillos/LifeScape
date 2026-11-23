#![forbid(unsafe_code)]
//! Rates page view (user 2026-11-17: the measures view published ~18 000
//! flow rows, one per (key, object): unusable). Pure: the HTTP layer
//! (`web_rates_view`) builds one `RatesIndex` from the live snapshot
//! (cached, never rebuilt per request) and every request only filters,
//! groups, sorts and pages it in memory.
//!
//! - index: one `KeyEntry` per MEASURED exact key (planner activity,
//!   style, site, tool, band; the roll-ups stay internal, R2.1) with its
//!   own rates (Σ / Σ work ticks × 6000), units, derived XP
//!   (`planner::unit_xp`), confidence, and the prices of its objects;
//! - rows: one per activity (method × style, `Group::Activity`, the
//!   default) or one per exact key (`Group::Key`). An activity row folds
//!   the keys the filters kept: each figure is Σ (key rate × key work
//!   ticks) / Σ key work ticks, i.e. Σ / Σ over its keys, never a mean of
//!   rates; runs and rejected samples add up; failure = Σ failed /
//!   Σ finished; age = its freshest key. A view figure only: the brain
//!   and the guides keep reading exact keys (`rates_read`);
//! - filters (key level, before grouping): text (method, style, site,
//!   tool, family, objects, skills), skill (derived XP > 0), family
//!   (method prefix: fight, gather, smith…), band, measured only
//!   (n >= 3, the estimator's threshold: below it the brain reads the
//!   prior), minimum runs;
//! - sort on any column server side, then the page (offset, limit) with
//!   the counts `matched` / `total`;
//! - detail: every flow of an activity or of one key (rare pickups
//!   folded into one valued "other drops" line, `guide_flows`), its
//!   units, XP, and its keys as sub-rows.

use std::collections::{BTreeMap, BTreeSet};

use crate::rates_estimator::{YieldBook, YieldKey};
use lifescape_planner::guide_flows::{fold_negligible_drops, OTHER_DROPS};

/// Runs from which a key counts as measured (`MIN_MEASURED_REPORTS`).
pub const MEASURED_RUNS: u64 = crate::rates_service::MIN_MEASURED_REPORTS as u64;
/// Rows of a page when the query gives no limit.
pub const DEFAULT_LIMIT: usize = 50;
/// Most rows a page serves.
pub const MAX_LIMIT: usize = 500;
/// Products and consumption lines a list row carries.
pub const HEADLINE_LINES: usize = 3;
/// Sort column when none (or an unknown one) is asked.
pub const DEFAULT_SORT: &str = "runs";

/// One measured exact key and its own figures.
#[derive(Clone, Debug)]
pub struct KeyEntry {
    pub key: YieldKey,
    pub family: String,
    pub runs: u64,
    pub work_ticks: f64,
    pub last_clock: i64,
    pub finished: f64,
    pub failed: f64,
    pub dispersion: f64,
    pub rejected: u64,
    /// (display item, cause, per 6000 work ticks), signed.
    pub flows: Vec<(String, String, f64)>,
    /// (unit kind, subject, per 6000).
    pub units: Vec<(String, String, f64)>,
    /// (skill, derived XP per 6000).
    pub xp: Vec<(String, f64)>,
    /// Lowercase text the search reads.
    haystack: String,
}

/// Every measured exact key, the prices of their objects and the clock
/// the ages are counted from.
#[derive(Clone, Debug, Default)]
pub struct RatesIndex {
    pub entries: Vec<KeyEntry>,
    pub prices: BTreeMap<String, f64>,
    pub now: i64,
}

/// Family of a planner method: its prefix (`fight`, `gather`, `smith`…).
pub fn family_of(method: &str) -> String {
    method.split(':').next().unwrap_or(method).to_string()
}

/// True when `display` is the activity's own product (never folded).
fn own_product(activity: &str, display: &str) -> bool {
    lifescape_planner::method_way::method_way(activity).is_some_and(|(product, _)| crate::rates_names::debug_of(display) == product)
}

/// The index of `book` at `now` (prices: display name -> coins, asked
/// once per object).
pub fn build_index(book: &YieldBook, now: i64, price: &mut dyn FnMut(&str) -> f64) -> RatesIndex {
    let mut prices: BTreeMap<String, f64> = BTreeMap::new();
    let mut entries = Vec::new();
    for (key, bucket) in book.buckets.iter().filter(|(key, bucket)| !key.is_rollup() && bucket.runs > 0) {
        let flows = bucket.flow_rates();
        for (item, _, _) in &flows {
            if !prices.contains_key(item) {
                let unit = price(item);
                prices.insert(item.clone(), unit);
            }
        }
        let units = bucket.unit_rates();
        let level = key.band.clamp(1, 99) as u8;
        let xp: Vec<(String, f64)> = lifescape_planner::unit_xp::derived_xp(&key.method, &key.style, &crate::rates_names::units_in_debug(&units), level).into_iter().filter(|(_, rate)| *rate > 0.0).collect();
        let family = family_of(&key.method);
        let mut words: Vec<&str> = vec![&key.method, &key.style, &key.site, &key.tool, &family];
        words.extend(flows.iter().map(|(item, _, _)| item.as_str()));
        words.extend(xp.iter().map(|(skill, _)| skill.as_str()));
        let haystack = words.join(" ").to_lowercase();
        entries.push(KeyEntry {
            key: key.clone(),
            family,
            runs: bucket.runs,
            work_ticks: bucket.work_ticks,
            last_clock: bucket.last_clock,
            finished: bucket.finished,
            failed: bucket.failed,
            dispersion: bucket.dispersion(),
            rejected: bucket.rejected,
            flows,
            units,
            xp,
            haystack,
        });
    }
    RatesIndex { entries, prices, now }
}

/// Rows of the view: one per activity or one per exact key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Group {
    #[default]
    Activity,
    Key,
}

/// What the page asks (all filters at key level, before grouping).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewQuery {
    pub group: Group,
    pub text: String,
    pub skill: Option<String>,
    pub family: Option<String>,
    pub band: Option<i64>,
    pub measured_only: bool,
    pub min_runs: u64,
    pub sort: String,
    pub descending: bool,
    pub offset: usize,
    pub limit: usize,
}

impl Default for ViewQuery {
    fn default() -> Self {
        ViewQuery { group: Group::Activity, text: String::new(), skill: None, family: None, band: None, measured_only: false, min_runs: 0, sort: DEFAULT_SORT.to_string(), descending: true, offset: 0, limit: DEFAULT_LIMIT }
    }
}

impl ViewQuery {
    fn keeps(&self, entry: &KeyEntry) -> bool {
        let needles = self.text.to_lowercase();
        if !needles.split_whitespace().all(|needle| entry.haystack.contains(needle)) {
            return false;
        }
        if self.skill.as_ref().is_some_and(|skill| !entry.xp.iter().any(|(known, _)| known == skill)) {
            return false;
        }
        if self.family.as_ref().is_some_and(|family| &entry.family != family) {
            return false;
        }
        if self.band.is_some_and(|band| entry.key.band != band) {
            return false;
        }
        if self.measured_only && entry.runs < MEASURED_RUNS {
            return false;
        }
        entry.runs >= self.min_runs
    }
}

/// One flow of a row, valued at the current price.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowLine {
    pub item: String,
    pub cause: String,
    pub per_6000: f64,
    pub value_6000: f64,
    /// Objects folded into this line ("other drops"), 0 for a plain one.
    pub folded: usize,
}

/// One row of the view (an activity, or one exact key).
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub method: String,
    pub style: String,
    pub family: String,
    /// Site, tool of a key row (None on an activity row).
    pub site: Option<String>,
    pub tool: Option<String>,
    pub band_min: i64,
    pub band_max: i64,
    pub keys: usize,
    pub measured_keys: usize,
    pub sites: usize,
    pub tools: Vec<String>,
    pub runs: u64,
    pub age_secs: i64,
    pub failure_rate: f64,
    /// Dispersion of a key row (an activity mixes keys: None).
    pub dispersion: Option<f64>,
    pub rejected: u64,
    /// Products first (best value first), then consumption (most costly
    /// first).
    pub flows: Vec<FlowLine>,
    pub units: Vec<(String, String, f64)>,
    /// Derived XP per skill, most first.
    pub xp: Vec<(String, f64)>,
    pub xp_total: f64,
    pub product_value_6000: f64,
    pub consumption_value_6000: f64,
    pub net_value_6000: f64,
}

impl Summary {
    /// Objects coming in (products, loot, pickups), best value first.
    pub fn products(&self) -> Vec<&FlowLine> {
        self.flows.iter().filter(|line| line.per_6000 > 0.0).collect()
    }

    /// Objects going out (inputs, consumables, fees, deaths).
    pub fn consumption(&self) -> Vec<&FlowLine> {
        self.flows.iter().filter(|line| line.per_6000 < 0.0).collect()
    }

    /// Share of measured keys (n >= 3): 1 = measured, 0 = thin.
    pub fn measured_share(&self) -> f64 {
        if self.keys == 0 { 0.0 } else { self.measured_keys as f64 / self.keys as f64 }
    }
}

/// Work-weighted sums of `(a, b) -> rate` maps over `entries`.
fn weighted<K: Ord + Clone>(entries: &[&KeyEntry], lines: &dyn Fn(&KeyEntry) -> Vec<(K, f64)>) -> Vec<(K, f64)> {
    let total: f64 = entries.iter().map(|entry| entry.work_ticks).sum();
    let mut sums: BTreeMap<K, f64> = BTreeMap::new();
    for entry in entries {
        for (name, rate) in lines(entry) {
            *sums.entry(name).or_insert(0.0) += rate * entry.work_ticks;
        }
    }
    sums.into_iter().map(|(name, sum)| (name, if total > 0.0 { sum / total } else { 0.0 })).collect()
}

/// The row of `entries` (all of one activity; one entry for a key row).
pub fn summarize(index: &RatesIndex, entries: &[&KeyEntry], key_row: bool) -> Summary {
    let first = &entries[0].key;
    let flows: Vec<(String, String, f64)> = weighted(entries, &|entry| entry.flows.iter().map(|(item, cause, rate)| ((item.clone(), cause.clone()), *rate)).collect()).into_iter().map(|((item, cause), rate)| (item, cause, rate)).collect();
    let units: Vec<(String, String, f64)> = weighted(entries, &|entry| entry.units.iter().map(|(kind, subject, rate)| ((kind.clone(), subject.clone()), *rate)).collect()).into_iter().map(|((kind, subject), rate)| (kind, subject, rate)).collect();
    let mut xp = weighted(entries, &|entry| entry.xp.clone());
    xp.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let unit_price = |item: &str| index.prices.get(item).copied().unwrap_or(0.0);
    let (kept, other) = fold_negligible_drops(&flows, &|item| own_product(&first.method, item), &unit_price);
    let mut lines: Vec<FlowLine> = kept.into_iter().map(|(item, cause, rate)| FlowLine { value_6000: rate * unit_price(&item), item, cause, per_6000: rate, folded: 0 }).collect();
    if other.items > 0 {
        lines.push(FlowLine { item: format!("{OTHER_DROPS} ({} objects)", other.items), cause: "loot".to_string(), per_6000: other.units_per_6000, value_6000: other.value_per_6000, folded: other.items });
    }
    // Products first, best value then most units; consumption after,
    // most costly first.
    lines.sort_by(|left, right| {
        let side = |line: &FlowLine| if line.per_6000 >= 0.0 { 0 } else { 1 };
        side(left).cmp(&side(right)).then_with(|| right.value_6000.abs().total_cmp(&left.value_6000.abs())).then_with(|| right.per_6000.abs().total_cmp(&left.per_6000.abs())).then_with(|| left.item.cmp(&right.item))
    });
    let product_value_6000: f64 = lines.iter().filter(|line| line.per_6000 > 0.0).map(|line| line.value_6000).sum();
    let consumption_value_6000: f64 = lines.iter().filter(|line| line.per_6000 < 0.0).map(|line| line.value_6000).sum();
    let finished: f64 = entries.iter().map(|entry| entry.finished).sum();
    let failed: f64 = entries.iter().map(|entry| entry.failed).sum();
    let last_clock = entries.iter().map(|entry| entry.last_clock).max().unwrap_or(0);
    let sites: BTreeSet<&str> = entries.iter().map(|entry| entry.key.site.as_str()).collect();
    let tools: BTreeSet<String> = entries.iter().map(|entry| entry.key.tool.clone()).collect();
    Summary {
        method: first.method.clone(),
        style: first.style.clone(),
        family: entries[0].family.clone(),
        site: key_row.then(|| first.site.clone()),
        tool: key_row.then(|| first.tool.clone()),
        band_min: entries.iter().map(|entry| entry.key.band).min().unwrap_or(0),
        band_max: entries.iter().map(|entry| entry.key.band).max().unwrap_or(0),
        keys: entries.len(),
        measured_keys: entries.iter().filter(|entry| entry.runs >= MEASURED_RUNS).count(),
        sites: sites.len(),
        tools: tools.into_iter().collect(),
        runs: entries.iter().map(|entry| entry.runs).sum(),
        age_secs: if last_clock > 0 { (index.now - last_clock).max(0) } else { 0 },
        failure_rate: if finished > 0.0 { failed / finished } else { 0.0 },
        dispersion: key_row.then(|| entries[0].dispersion),
        rejected: entries.iter().map(|entry| entry.rejected).sum(),
        xp_total: xp.iter().map(|(_, rate)| rate).sum(),
        flows: lines,
        units,
        xp,
        product_value_6000,
        consumption_value_6000,
        net_value_6000: product_value_6000 + consumption_value_6000,
    }
}

/// Entries the query keeps, grouped as its rows (BTreeMap: stable order).
fn groups<'a>(query: &ViewQuery, entries: impl Iterator<Item = &'a KeyEntry>) -> Vec<Vec<&'a KeyEntry>> {
    let mut grouped: BTreeMap<(String, String, String, String, i64), Vec<&KeyEntry>> = BTreeMap::new();
    for entry in entries {
        let key = &entry.key;
        let id = match query.group {
            Group::Activity => (key.method.clone(), key.style.clone(), String::new(), String::new(), 0),
            Group::Key => (key.method.clone(), key.style.clone(), key.site.clone(), key.tool.clone(), key.band),
        };
        grouped.entry(id).or_default().push(entry);
    }
    grouped.into_values().collect()
}

/// Sort value of a row column: a number, or a text.
enum SortValue {
    Number(f64),
    Text(String),
}

/// Columns the page sorts on (anything else: `DEFAULT_SORT`).
pub const SORT_COLUMNS: [&str; 16] = [
    "method", "family", "site", "tool", "band", "keys", "runs", "age_secs", "confidence", "failure_rate", "dispersion", "rejected", "product_value_6000", "consumption_value_6000", "net_value_6000", "xp_total",
];

fn sort_value(row: &Summary, column: &str) -> SortValue {
    match column {
        "method" => SortValue::Text(format!("{} {}", row.method, row.style)),
        "family" => SortValue::Text(row.family.clone()),
        "site" => SortValue::Text(row.site.clone().unwrap_or_default()),
        "tool" => SortValue::Text(row.tool.clone().unwrap_or_else(|| row.tools.join(","))),
        "band" => SortValue::Number(row.band_min as f64),
        "keys" => SortValue::Number(row.keys as f64),
        "age_secs" => SortValue::Number(row.age_secs as f64),
        "confidence" => SortValue::Number(row.measured_share() * 1e9 + row.runs as f64),
        "failure_rate" => SortValue::Number(row.failure_rate),
        "dispersion" => SortValue::Number(row.dispersion.unwrap_or(0.0)),
        "rejected" => SortValue::Number(row.rejected as f64),
        "product_value_6000" => SortValue::Number(row.product_value_6000),
        "consumption_value_6000" => SortValue::Number(row.consumption_value_6000),
        "net_value_6000" => SortValue::Number(row.net_value_6000),
        "xp_total" => SortValue::Number(row.xp_total),
        _ => SortValue::Number(row.runs as f64),
    }
}

fn compare(left: &Summary, right: &Summary, column: &str, descending: bool) -> std::cmp::Ordering {
    let order = match (sort_value(left, column), sort_value(right, column)) {
        (SortValue::Number(a), SortValue::Number(b)) => a.total_cmp(&b),
        (SortValue::Text(a), SortValue::Text(b)) => a.cmp(&b),
        _ => std::cmp::Ordering::Equal,
    };
    let order = if descending { order.reverse() } else { order };
    order.then_with(|| (&left.method, &left.style, left.band_min, &left.site, &left.tool).cmp(&(&right.method, &right.style, right.band_min, &right.site, &right.tool)))
}

/// One page of rows and its counts.
#[derive(Clone, Debug)]
pub struct Page {
    /// Rows of the group with no filter.
    pub total: usize,
    /// Rows the filters keep.
    pub matched: usize,
    pub offset: usize,
    pub limit: usize,
    /// Column actually sorted on, and its direction.
    pub sort: String,
    pub descending: bool,
    pub rows: Vec<Summary>,
}

/// The page `query` asks of `index`.
pub fn query_page(index: &RatesIndex, query: &ViewQuery) -> Page {
    let total = groups(&ViewQuery { group: query.group, ..ViewQuery::default() }, index.entries.iter()).len();
    let mut rows: Vec<Summary> = groups(query, index.entries.iter().filter(|entry| query.keeps(entry))).iter().map(|entries| summarize(index, entries, query.group == Group::Key)).collect();
    let sort = if SORT_COLUMNS.contains(&query.sort.as_str()) { query.sort.clone() } else { DEFAULT_SORT.to_string() };
    rows.sort_by(|left, right| compare(left, right, &sort, query.descending));
    let matched = rows.len();
    let limit = if query.limit == 0 { DEFAULT_LIMIT } else { query.limit.min(MAX_LIMIT) };
    let rows: Vec<Summary> = rows.into_iter().skip(query.offset).take(limit).collect();
    Page { total, matched, offset: query.offset, limit, sort, descending: query.descending, rows }
}

/// What a detail asks: an activity, narrowed to a key by the optional
/// site, tool and band.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DetailTarget {
    pub method: String,
    pub style: String,
    pub site: Option<String>,
    pub tool: Option<String>,
    pub band: Option<i64>,
}

/// Detail of an activity (or a key): its folded row with every flow,
/// and each of its keys as a sub-row.
#[derive(Clone, Debug)]
pub struct Detail {
    pub summary: Summary,
    pub keys: Vec<Summary>,
}

/// The detail of `target` under the page's filters (`query`: text,
/// skill, family, band, measured only, minimum runs), None when no key
/// of the target passes them.
pub fn query_detail(index: &RatesIndex, target: &DetailTarget, query: &ViewQuery) -> Option<Detail> {
    let entries: Vec<&KeyEntry> = index
        .entries
        .iter()
        .filter(|entry| entry.key.method == target.method && entry.key.style == target.style)
        .filter(|entry| target.site.as_ref().is_none_or(|site| &entry.key.site == site))
        .filter(|entry| target.tool.as_ref().is_none_or(|tool| &entry.key.tool == tool))
        .filter(|entry| target.band.is_none_or(|band| entry.key.band == band))
        .filter(|entry| query.keeps(entry))
        .collect();
    if entries.is_empty() {
        return None;
    }
    let summary = summarize(index, &entries, entries.len() == 1);
    let mut keys: Vec<Summary> = entries.iter().map(|entry| summarize(index, &[*entry], true)).collect();
    keys.sort_by(|left, right| (left.band_min, &left.site, &left.tool).cmp(&(right.band_min, &right.site, &right.tool)));
    Some(Detail { summary, keys })
}

/// Values the filters take in the index (selects of the page).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facets {
    pub families: Vec<String>,
    pub skills: Vec<String>,
    pub bands: Vec<i64>,
}

pub fn facets(index: &RatesIndex) -> Facets {
    let families: BTreeSet<String> = index.entries.iter().map(|entry| entry.family.clone()).collect();
    let skills: BTreeSet<String> = index.entries.iter().flat_map(|entry| entry.xp.iter().map(|(skill, _)| skill.clone())).collect();
    let bands: BTreeSet<i64> = index.entries.iter().map(|entry| entry.key.band).filter(|band| *band > 0).collect();
    Facets { families: families.into_iter().collect(), skills: skills.into_iter().collect(), bands: bands.into_iter().collect() }
}

#[cfg(test)]
#[path = "rates_view_tests.rs"]
mod tests;
