#![forbid(unsafe_code)]
//! Errors page maths (pure): KPIs, hourly trend, breakdowns and
//! signature groups over the rows of one window. The web layer loads the
//! rows (`web_errors`), this module only counts.

use std::collections::{BTreeMap, HashMap, HashSet};

use lifescape_json::{object, str_value, Value};
use lifescape_store::{ErrorFilter, ErrorRecord};

/// Sources in display order (chart series, KPI tiles).
pub const SOURCES: [&str; 4] = ["task", "nav", "deal", "brain"];
/// Breakdown rows kept per table.
const TOP_KEYS: usize = 25;
/// Signature groups kept in the summary.
const TOP_GROUPS: usize = 200;

/// Everything one summary needs, rows already filtered to the window.
pub struct SummaryInput<'a> {
    pub rows: &'a [ErrorRecord],
    pub since: i64,
    pub until: i64,
    /// Same-length window just before `since`, same filter.
    pub previous_total: i64,
    /// Finished task reports per (script, stop) in the window.
    pub runs: &'a [(String, String, i64)],
    /// (settled, failed) deals in the window.
    pub deals: (i64, i64),
}

/// `part / whole`, None when nothing to divide.
pub fn rate(part: i64, whole: i64) -> Option<f64> {
    if whole <= 0 {
        None
    } else {
        Some(part as f64 / whole as f64)
    }
}

/// Change in percent against the previous window, None without one.
pub fn trend_pct(current: i64, previous: i64) -> Option<f64> {
    rate(current - previous, previous).map(|ratio| (ratio * 1000.0).round() / 10.0)
}

fn float_or_null(value: Option<f64>) -> Value {
    value.map(Value::Float).unwrap_or(Value::Null)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Same matching as the store filter (window included), for rows
/// already loaded: the summary filters in memory once per request.
pub fn row_matches(row: &ErrorRecord, filter: &ErrorFilter) -> bool {
    let exact = |wanted: &str, value: &str| wanted.is_empty() || wanted == value;
    row.clock >= filter.since
        && (filter.until <= 0 || row.clock < filter.until)
        && exact(&filter.source, &row.source)
        && exact(&filter.kind, &row.kind)
        && exact(&filter.script, &row.script)
        && exact(&filter.zone, &row.zone)
        && exact(&filter.signature, &row.signature)
        && (filter.account.is_empty() || filter.account == row.account || filter.account == row.peer)
}

fn bump(counter: &mut HashMap<String, i64>, key: &str) {
    *counter.entry(key.to_string()).or_insert(0) += 1;
}

/// Counter to `[{key, count}]`, most frequent first (ties by key).
fn ranked(counter: HashMap<String, i64>, keep: usize) -> Vec<(String, i64)> {
    let mut pairs: Vec<(String, i64)> = counter.into_iter().collect();
    pairs.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    pairs.truncate(keep);
    pairs
}

fn ranked_value(counter: HashMap<String, i64>, keep: usize) -> Value {
    Value::Arr(
        ranked(counter, keep)
            .into_iter()
            .map(|(key, count)| object(vec![("key", str_value(&key)), ("count", Value::Int(count))]))
            .collect(),
    )
}

fn top_key(counter: &HashMap<String, i64>) -> String {
    ranked(counter.clone(), 1).into_iter().next().map(|(key, _)| key).unwrap_or_default()
}

/// Hourly buckets covering [since, until): `{t, total, <source>…}`,
/// zero-filled (an hour without errors is a real zero).
pub fn hourly(rows: &[ErrorRecord], since: i64, until: i64) -> Vec<Value> {
    let first = since.div_euclid(3600) * 3600;
    let mut buckets: BTreeMap<i64, HashMap<String, i64>> = BTreeMap::new();
    let mut start = first;
    while start < until {
        buckets.insert(start, HashMap::new());
        start += 3600;
    }
    for row in rows {
        let slot = row.clock.div_euclid(3600) * 3600;
        if let Some(counts) = buckets.get_mut(&slot) {
            bump(counts, &row.source);
        }
    }
    buckets
        .into_iter()
        .map(|(start, counts)| {
            let mut entry = BTreeMap::new();
            entry.insert("t".to_string(), Value::Int(start));
            entry.insert("total".to_string(), Value::Int(counts.values().sum()));
            for source in SOURCES {
                entry.insert(source.to_string(), Value::Int(counts.get(source).copied().unwrap_or(0)));
            }
            Value::Obj(entry)
        })
        .collect()
}

#[derive(Default)]
struct Group {
    count: i64,
    accounts: HashSet<String>,
    account_counts: HashMap<String, i64>,
    zones: HashMap<String, i64>,
    scripts: HashMap<String, i64>,
    first_seen: i64,
    last_seen: i64,
    last_id: i64,
    last_account: String,
}

/// Groups by (source, kind, signature), most frequent first.
pub fn groups(rows: &[ErrorRecord], total: i64) -> Vec<Value> {
    let mut grouped: HashMap<(String, String, String), Group> = HashMap::new();
    for row in rows {
        let group = grouped.entry((row.source.clone(), row.kind.clone(), row.signature.clone())).or_default();
        if group.count == 0 || row.clock < group.first_seen {
            group.first_seen = row.clock;
        }
        if group.count == 0 || row.clock >= group.last_seen {
            group.last_seen = row.clock;
            group.last_id = row.error_id;
            group.last_account = row.account.clone();
        }
        group.count += 1;
        if !row.account.is_empty() {
            group.accounts.insert(row.account.clone());
            bump(&mut group.account_counts, &row.account);
        }
        if !row.zone.is_empty() {
            bump(&mut group.zones, &row.zone);
        }
        if !row.script.is_empty() {
            bump(&mut group.scripts, &row.script);
        }
    }
    let mut ordered: Vec<((String, String, String), Group)> = grouped.into_iter().collect();
    ordered.sort_by(|left, right| right.1.count.cmp(&left.1.count).then_with(|| right.1.last_seen.cmp(&left.1.last_seen)));
    ordered.truncate(TOP_GROUPS);
    ordered
        .into_iter()
        .map(|((source, kind, signature), group)| {
            let top_accounts: Vec<Value> = ranked(group.account_counts.clone(), 3).into_iter().map(|(key, _)| str_value(&key)).collect();
            object(vec![
                ("source", str_value(&source)),
                ("kind", str_value(&kind)),
                ("signature", str_value(&signature)),
                ("count", Value::Int(group.count)),
                ("share", float_or_null(rate(group.count, total).map(round3))),
                ("accounts", Value::Int(group.accounts.len() as i64)),
                ("top_accounts", Value::Arr(top_accounts)),
                ("top_zone", str_value(&top_key(&group.zones))),
                ("top_script", str_value(&top_key(&group.scripts))),
                ("first_seen", Value::Int(group.first_seen)),
                ("last_seen", Value::Int(group.last_seen)),
                ("last_account", str_value(&group.last_account)),
                ("last_id", Value::Int(group.last_id)),
            ])
        })
        .collect()
}

/// Per-script runs (task reports) against failures: failure rate =
/// failed / runs, block rate = blocked / runs, plus error rows seen.
pub fn script_table(rows: &[ErrorRecord], runs: &[(String, String, i64)]) -> Vec<Value> {
    let mut table: BTreeMap<String, (i64, i64, i64, i64)> = BTreeMap::new();
    for (script, stop, count) in runs {
        let entry = table.entry(script.clone()).or_default();
        entry.0 += count;
        if stop == "failed" {
            entry.1 += count;
        } else if stop == "blocked" {
            entry.2 += count;
        }
    }
    for row in rows.iter().filter(|row| !row.script.is_empty()) {
        table.entry(row.script.clone()).or_default().3 += 1;
    }
    let mut lines: Vec<(String, (i64, i64, i64, i64))> = table.into_iter().collect();
    lines.sort_by(|left, right| right.1 .1.cmp(&left.1 .1).then_with(|| right.1 .3.cmp(&left.1 .3)).then_with(|| left.0.cmp(&right.0)));
    lines
        .into_iter()
        .map(|(script, (total, failed, blocked, errors))| {
            object(vec![
                ("script", str_value(&script)),
                ("runs", Value::Int(total)),
                ("failed", Value::Int(failed)),
                ("blocked", Value::Int(blocked)),
                ("failure_rate", float_or_null(rate(failed, total).map(round3))),
                ("block_rate", float_or_null(rate(blocked, total).map(round3))),
                ("errors", Value::Int(errors)),
            ])
        })
        .collect()
}

/// Distinct filter values (from the unfiltered window rows).
pub fn filter_options(rows: &[ErrorRecord]) -> Value {
    let mut sets: [std::collections::BTreeSet<String>; 5] = Default::default();
    for row in rows {
        for (index, value) in [&row.source, &row.kind, &row.account, &row.script, &row.zone].into_iter().enumerate() {
            if !value.is_empty() {
                sets[index].insert(value.clone());
            }
        }
        if !row.peer.is_empty() {
            sets[2].insert(row.peer.clone());
        }
    }
    let names = ["sources", "kinds", "accounts", "scripts", "zones"];
    let mut map = BTreeMap::new();
    for (name, set) in names.into_iter().zip(sets) {
        map.insert(name.to_string(), Value::Arr(set.iter().map(|value| str_value(value)).collect()));
    }
    Value::Obj(map)
}

/// The summary payload body (without `ok`).
pub fn summarize(input: &SummaryInput<'_>) -> Vec<(&'static str, Value)> {
    let rows = input.rows;
    let total = rows.len() as i64;
    let hours = ((input.until - input.since) as f64 / 3600.0).max(1.0 / 60.0);
    let mut by_source: HashMap<String, i64> = HashMap::new();
    let mut by_kind: HashMap<String, i64> = HashMap::new();
    let mut by_account: HashMap<String, i64> = HashMap::new();
    let mut by_script: HashMap<String, i64> = HashMap::new();
    let mut by_zone: HashMap<String, i64> = HashMap::new();
    let mut signatures: HashSet<(&str, &str, &str)> = HashSet::new();
    let mut located = 0;
    for row in rows {
        bump(&mut by_source, &row.source);
        bump(&mut by_kind, &format!("{}/{}", row.source, row.kind));
        if !row.account.is_empty() {
            bump(&mut by_account, &row.account);
        }
        if !row.peer.is_empty() {
            bump(&mut by_account, &row.peer);
        }
        if !row.script.is_empty() {
            bump(&mut by_script, &row.script);
        }
        if !row.zone.is_empty() {
            bump(&mut by_zone, &row.zone);
        }
        if row.position.is_some() {
            located += 1;
        }
        signatures.insert((&row.source, &row.kind, &row.signature));
    }
    let accounts_hit = by_account.len() as i64;
    let (runs_total, runs_failed, runs_blocked) = input.runs.iter().fold((0, 0, 0), |(all, failed, blocked), (_, stop, count)| {
        (all + count, failed + if stop == "failed" { *count } else { 0 }, blocked + if stop == "blocked" { *count } else { 0 })
    });
    let (deals_settled, deals_failed) = input.deals;
    let mut kpis = vec![
        ("total", Value::Int(total)),
        ("per_hour", Value::Float((total as f64 / hours * 10.0).round() / 10.0)),
        ("previous_total", Value::Int(input.previous_total)),
        ("trend_pct", float_or_null(trend_pct(total, input.previous_total))),
        ("accounts", Value::Int(accounts_hit)),
        ("signatures", Value::Int(signatures.len() as i64)),
        ("located", Value::Int(located)),
        ("runs", Value::Int(runs_total)),
        ("runs_failed", Value::Int(runs_failed)),
        ("runs_blocked", Value::Int(runs_blocked)),
        ("run_failure_rate", float_or_null(rate(runs_failed, runs_total).map(round3))),
        ("deals_settled", Value::Int(deals_settled)),
        ("deals_failed", Value::Int(deals_failed)),
        ("deal_failure_rate", float_or_null(rate(deals_failed, deals_settled + deals_failed).map(round3))),
    ];
    for source in SOURCES {
        kpis.push((source, Value::Int(by_source.get(source).copied().unwrap_or(0))));
    }
    vec![
        ("since", Value::Int(input.since)),
        ("until", Value::Int(input.until)),
        ("kpis", object(kpis)),
        ("hourly", Value::Arr(hourly(rows, input.since, input.until))),
        ("by_source", ranked_value(by_source, TOP_KEYS)),
        ("by_kind", ranked_value(by_kind, TOP_KEYS)),
        ("by_account", ranked_value(by_account, TOP_KEYS)),
        ("by_script", ranked_value(by_script, TOP_KEYS)),
        ("by_zone", ranked_value(by_zone, TOP_KEYS)),
        ("groups", Value::Arr(groups(rows, total))),
        ("scripts", Value::Arr(script_table(rows, input.runs))),
    ]
}

#[cfg(test)]
#[path = "errors_aggregate_tests.rs"]
mod tests;
