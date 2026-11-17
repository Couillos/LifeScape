#![forbid(unsafe_code)]
//! Rates page HTTP (user 2026-11-17): the aggregated, filtered, paged
//! view of the measures (`rates_view`), never the ~18 000 raw flow rows.
//!
//! - `GET /api/rates/activities?group=activity|key&q=&skill=&family=&band=
//!   &measured=1&min_runs=&sort=&dir=asc|desc&offset=&limit=`: one page of
//!   rows (headline products and consumption, derived XP, confidence),
//!   the counts `matched` / `total`, the facets of the filters;
//! - `GET /api/rates/detail?method=&style=[&site=&tool=&band=]` (plus the
//!   page's filters): every flow, unit and XP line of an activity or of
//!   one key, and its keys as sub-rows;
//! - `GET /api/rates/trading`: the trading table and the engaged bots
//!   (operator eyes only), without the measure rows.
//!
//! The index is built from the live snapshot and the store prices at most
//! once per `INDEX_TTL` while reports land (`rates_service::generation`),
//! else kept up to `PRICE_TTL` (prices move with the market); a request
//! only filters, groups, sorts and pages it. Each reply carries the time
//! it took (`took_ms`) and the index's own cost and age.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lifescape_json::{object, str_value, Value};

use crate::rates_view::{build_index, facets, query_detail, query_page, DetailTarget, FlowLine, Group, RatesIndex, Summary, ViewQuery, HEADLINE_LINES};
use crate::web::Response;

/// Least age of an index rebuilt for new reports.
pub const INDEX_TTL: Duration = Duration::from_secs(30);
/// Most age of an index (prices move even without a new report).
pub const PRICE_TTL: Duration = Duration::from_secs(300);

/// One built index and what it was built from.
pub struct CachedIndex {
    pub built: Instant,
    pub generation: u64,
    pub build_ms: f64,
    pub index: Arc<RatesIndex>,
}

impl CachedIndex {
    /// Serves under `INDEX_TTL`, and up to `PRICE_TTL` while no report
    /// landed since its build.
    pub fn fresh(&self, now: Instant, generation: u64) -> bool {
        let age = now.saturating_duration_since(self.built);
        age < INDEX_TTL || (self.generation == generation && age < PRICE_TTL)
    }
}

fn cache() -> &'static Mutex<Option<CachedIndex>> {
    static CACHE: OnceLock<Mutex<Option<CachedIndex>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The live index (built under the lock: concurrent callers wait for one
/// build), with its build cost and age in ms; an error when the store
/// does not open.
fn live_index() -> Result<(Arc<RatesIndex>, f64, f64), String> {
    let mut slot = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    let generation = crate::rates_service::generation();
    if let Some(cached) = slot.as_ref().filter(|cached| cached.fresh(now, generation)) {
        return Ok((cached.index.clone(), cached.build_ms, now.saturating_duration_since(cached.built).as_secs_f64() * 1000.0));
    }
    let store = lifescape_store::Store::open(&crate::paths::store_db()).map_err(|error| format!("rates store: {error}"))?;
    let snapshot = crate::rates_service::live_snapshot();
    let index = Arc::new(index_of(&store, &snapshot));
    let build_ms = now.elapsed().as_secs_f64() * 1000.0;
    *slot = Some(CachedIndex { built: now, generation, build_ms, index: index.clone() });
    Ok((index, build_ms, 0.0))
}

/// The index of `snapshot`, objects priced through `store` (current
/// price, `rates_service::price_estimate`).
pub fn index_of(store: &lifescape_store::Store, snapshot: &crate::rates_service::Snapshot) -> RatesIndex {
    build_index(snapshot.book(), crate::rates_service::now_secs(), &mut |item: &str| crate::rates_service::price_estimate(store, item).0)
}

/// The view query of the URL parameters (decoded).
pub fn view_query_of(params: &HashMap<String, String>) -> ViewQuery {
    let text = |key: &str| params.get(key).map(|value| crate::web_errors::percent_decode(value).trim().to_string()).filter(|value| !value.is_empty());
    let number = |key: &str| text(key).and_then(|value| value.parse::<u64>().ok());
    ViewQuery {
        group: if text("group").as_deref() == Some("key") { Group::Key } else { Group::Activity },
        text: text("q").unwrap_or_default(),
        skill: text("skill"),
        family: text("family"),
        band: text("band").and_then(|value| value.parse::<i64>().ok()).filter(|band| *band > 0),
        measured_only: matches!(text("measured").as_deref(), Some("1" | "true" | "yes")),
        min_runs: number("min_runs").unwrap_or(0),
        sort: text("sort").unwrap_or_else(|| crate::rates_view::DEFAULT_SORT.to_string()),
        descending: text("dir").as_deref() != Some("asc"),
        offset: number("offset").unwrap_or(0) as usize,
        limit: number("limit").unwrap_or(0) as usize,
    }
}

fn line_value(line: &FlowLine) -> Value {
    object(vec![
        ("item", str_value(&line.item)),
        ("cause", str_value(&line.cause)),
        ("per_6000", Value::Float(line.per_6000)),
        ("value_6000", Value::Float(line.value_6000)),
        ("folded", Value::Int(line.folded as i64)),
    ])
}

fn lines_value<'a>(lines: impl Iterator<Item = &'a FlowLine>) -> Value {
    Value::Arr(lines.map(line_value).collect())
}

/// One row: key and confidence columns, headline flows (`full`: every
/// flow, unit and XP line).
pub fn summary_value(row: &Summary, full: bool) -> Value {
    let optional = |value: &Option<String>| value.as_ref().map(|text| str_value(text)).unwrap_or(Value::Null);
    let mut xp = BTreeMap::new();
    for (skill, rate) in &row.xp {
        xp.insert(skill.clone(), Value::Float(*rate));
    }
    let mut fields = vec![
        ("method", str_value(&row.method)),
        ("style", str_value(&row.style)),
        ("family", str_value(&row.family)),
        ("site", optional(&row.site)),
        ("tool", optional(&row.tool)),
        ("band", Value::Int(row.band_min)),
        ("band_max", Value::Int(row.band_max)),
        ("keys", Value::Int(row.keys as i64)),
        ("measured_keys", Value::Int(row.measured_keys as i64)),
        ("confidence", Value::Float(row.measured_share())),
        ("thin", Value::Bool(row.measured_keys == 0)),
        ("sites", Value::Int(row.sites as i64)),
        ("tools", Value::Arr(row.tools.iter().map(|tool| str_value(tool)).collect())),
        ("runs", Value::Int(row.runs as i64)),
        ("age_secs", Value::Int(row.age_secs)),
        ("failure_rate", Value::Float(row.failure_rate)),
        ("dispersion", row.dispersion.map(Value::Float).unwrap_or(Value::Null)),
        ("rejected", Value::Int(row.rejected as i64)),
        ("xp", Value::Obj(xp)),
        ("xp_total", Value::Float(row.xp_total)),
        ("product_value_6000", Value::Float(row.product_value_6000)),
        ("consumption_value_6000", Value::Float(row.consumption_value_6000)),
        ("net_value_6000", Value::Float(row.net_value_6000)),
    ];
    if full {
        fields.push(("flows", lines_value(row.flows.iter())));
        fields.push(("units", Value::Arr(row.units.iter().map(|(kind, subject, rate)| object(vec![("kind", str_value(kind)), ("subject", str_value(subject)), ("per_6000", Value::Float(*rate))])).collect())));
    } else {
        fields.push(("products", lines_value(row.products().into_iter().take(HEADLINE_LINES))));
        fields.push(("consumption", lines_value(row.consumption().into_iter().take(HEADLINE_LINES))));
        fields.push(("flow_count", Value::Int(row.flows.len() as i64)));
    }
    object(fields)
}

fn timing(fields: &mut Vec<(&'static str, Value)>, began: Instant, build_ms: f64, age_ms: f64) {
    fields.push(("took_ms", Value::Float(began.elapsed().as_secs_f64() * 1000.0)));
    fields.push(("index_build_ms", Value::Float(build_ms)));
    fields.push(("index_age_ms", Value::Float(age_ms)));
}

/// `/api/rates/activities` on `index`.
pub fn activities_value(index: &RatesIndex, query: &ViewQuery) -> Vec<(&'static str, Value)> {
    let page = query_page(index, query);
    let facets = facets(index);
    vec![
        ("ok", Value::Bool(true)),
        ("group", str_value(if query.group == Group::Key { "key" } else { "activity" })),
        ("total", Value::Int(page.total as i64)),
        ("matched", Value::Int(page.matched as i64)),
        ("offset", Value::Int(page.offset as i64)),
        ("limit", Value::Int(page.limit as i64)),
        ("sort", str_value(&page.sort)),
        ("dir", str_value(if page.descending { "desc" } else { "asc" })),
        ("keys_indexed", Value::Int(index.entries.len() as i64)),
        ("rows", Value::Arr(page.rows.iter().map(|row| summary_value(row, false)).collect())),
        (
            "facets",
            object(vec![
                ("families", Value::Arr(facets.families.iter().map(|family| str_value(family)).collect())),
                ("skills", Value::Arr(facets.skills.iter().map(|skill| str_value(skill)).collect())),
                ("bands", Value::Arr(facets.bands.iter().map(|band| Value::Int(*band)).collect())),
            ]),
        ),
    ]
}

/// `/api/rates/detail` on `index`: the target's folded row and its keys.
pub fn detail_value(index: &RatesIndex, params: &HashMap<String, String>) -> Vec<(&'static str, Value)> {
    let text = |key: &str| params.get(key).map(|value| crate::web_errors::percent_decode(value));
    let target = DetailTarget {
        method: text("method").unwrap_or_default(),
        style: text("style").unwrap_or_default(),
        site: text("site").filter(|site| !site.is_empty()),
        tool: text("tool").filter(|tool| !tool.is_empty()),
        band: text("band").and_then(|band| band.parse::<i64>().ok()),
    };
    // The page's filters, never its band: a detail's band is the key's.
    let query = ViewQuery { band: None, ..view_query_of(params) };
    match query_detail(index, &target, &query) {
        Some(detail) => vec![
            ("ok", Value::Bool(true)),
            ("summary", summary_value(&detail.summary, true)),
            ("keys", Value::Arr(detail.keys.iter().map(|key| summary_value(key, false)).collect())),
        ],
        None => vec![("ok", Value::Bool(false)), ("error", str_value(&format!("rates: no measured key of {} {} under these filters", target.method, target.style)))],
    }
}

/// Routes of the rates page API (`None`: not ours).
pub(crate) fn route(parts: &[&str], query: &str, call: &dyn Fn(Value) -> Value) -> Option<Response> {
    let began = Instant::now();
    let params = crate::web::parse_query(query);
    let reply = match parts {
        ["api", "rates", "activities"] | ["api", "rates", "detail"] => match live_index() {
            Ok((index, build_ms, age_ms)) => {
                let mut fields = if parts[2] == "detail" { detail_value(&index, &params) } else { activities_value(&index, &view_query_of(&params)) };
                timing(&mut fields, began, build_ms, age_ms);
                object(fields)
            }
            Err(error) => crate::web::err_json(error),
        },
        ["api", "rates", "trading"] => crate::rates_service::trading_api(call),
        _ => return None,
    };
    Some(crate::web::ok_json(reply))
}

#[cfg(test)]
#[path = "web_rates_view_tests.rs"]
mod tests;
