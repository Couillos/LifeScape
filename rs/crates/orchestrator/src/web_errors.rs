//! Errors page: centralised error log over the `error_events` table
//! (fed by `errors_ingest`), plus the run and deal counts behind the
//! failure rates.
//!
//! Routes: GET /errors /errors.js
//! /api/errors/summary?hours=&since=&until=&source=&type=&account=&script=&zone=&signature=
//!   → `{ok, since, until, hours, kpis, hourly, by_source, by_kind,
//!      by_account, by_script, by_zone, groups, scripts, options}`
//! /api/errors/list?<same filters>&limit= → `{ok, total, rows:[…]}`
//! /api/errors/dossier/<account>-<tick>.json → the raw nav dossier.
//! `type` is an alias of `kind`; `hours` defaults to 24 (1..=168).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use lifescape_json::{self as json, object, str_value, Value};
use lifescape_store::{ErrorFilter, ErrorRecord, Store};

use crate::errors_aggregate::{self, SummaryInput};
use crate::web::{err_json, ok_json, parse_query, Response};

const DEFAULT_HOURS: i64 = 24;
const MAX_HOURS: i64 = crate::errors_ingest::RETENTION_SECS / 3600;
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 2_000;

/// Decode one query value: `+` is a space, `%XX` a byte (UTF-8 lossy).
pub(crate) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|text| u8::from_str_radix(text, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn int_param(params: &HashMap<String, String>, key: &str) -> Result<Option<i64>, String> {
    match params.get(key).map(String::as_str).filter(|raw| !raw.is_empty()) {
        None => Ok(None),
        Some(raw) => raw.parse::<i64>().map(Some).map_err(|_| format!("bad {key} '{raw}'")),
    }
}

/// Filter from the query: window (`since`/`until` unix seconds, else the
/// last `hours`), exact-match fields, list limit. Returns the hours span.
pub(crate) fn parse_filter(query: &str, now: i64) -> Result<(ErrorFilter, i64), String> {
    let params: HashMap<String, String> =
        parse_query(query).into_iter().map(|(key, value)| (key, percent_decode(&value))).collect();
    let hours = int_param(&params, "hours")?.unwrap_or(DEFAULT_HOURS);
    if !(1..=MAX_HOURS).contains(&hours) {
        return Err(format!("bad hours {hours}: want 1..={MAX_HOURS}"));
    }
    let until = int_param(&params, "until")?.unwrap_or(now);
    let since = int_param(&params, "since")?.unwrap_or(until - hours * 3600);
    if since >= until {
        return Err(format!("empty window since={since} until={until}"));
    }
    let limit = int_param(&params, "limit")?.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(format!("bad limit {limit}: want 1..={MAX_LIMIT}"));
    }
    let text = |key: &str| params.get(key).map(|value| value.trim().to_string()).unwrap_or_default();
    let kind = if text("kind").is_empty() { text("type") } else { text("kind") };
    let filter = ErrorFilter {
        since,
        until,
        source: text("source"),
        kind,
        account: text("account"),
        script: text("script"),
        zone: text("zone"),
        signature: text("signature"),
        limit,
    };
    let span_hours = ((until - since) + 3599) / 3600;
    Ok((filter, span_hours))
}

fn open_store(run_dir: &Path) -> Result<Store, String> {
    Store::open(&run_dir.join("lifescape.db")).map_err(|error| format!("errors store: {error}"))
}

/// Summary body: KPIs, hourly trend, breakdowns, groups, script rates,
/// and the filter options of the unfiltered window.
pub(crate) fn summary_value(store: &Store, filter: &ErrorFilter, hours: i64) -> Result<Value, String> {
    let window = ErrorFilter { since: filter.since, until: filter.until, ..ErrorFilter::default() };
    let all = store.errors_slim(&window).map_err(|error| error.to_string())?;
    let rows: Vec<ErrorRecord> = all.iter().filter(|row| errors_aggregate::row_matches(row, filter)).cloned().collect();
    let span = filter.until - filter.since;
    let previous = ErrorFilter { since: filter.since - span, until: filter.since, limit: 0, ..filter.clone() };
    let previous_total = store.count_errors(&previous).map_err(|error| error.to_string())?;
    let mut runs = store.task_stop_counts(filter.since, filter.until).map_err(|error| error.to_string())?;
    if !filter.script.is_empty() {
        runs.retain(|(script, _, _)| *script == filter.script);
    }
    let deals = store.deal_outcome_counts(filter.since, filter.until).map_err(|error| error.to_string())?;
    let mut body = vec![("ok", Value::Bool(true)), ("hours", Value::Int(hours))];
    body.extend(errors_aggregate::summarize(&SummaryInput {
        rows: &rows,
        since: filter.since,
        until: filter.until,
        previous_total,
        runs: &runs,
        deals,
    }));
    body.push(("options", errors_aggregate::filter_options(&all)));
    body.push(("retention_hours", Value::Int(MAX_HOURS)));
    Ok(object(body))
}

/// One list row for the drill-down table.
pub(crate) fn row_value(row: &ErrorRecord) -> Value {
    object(vec![
        ("id", Value::Int(row.error_id)),
        ("clock", Value::Int(row.clock)),
        ("time", str_value(&crate::errors_model::utc_text(row.clock))),
        ("source", str_value(&row.source)),
        ("kind", str_value(&row.kind)),
        ("account", str_value(&row.account)),
        ("peer", str_value(&row.peer)),
        ("script", str_value(&row.script)),
        ("args", str_value(&row.args)),
        ("node_id", str_value(&row.node_id)),
        ("method", str_value(&row.method)),
        ("stop", str_value(&row.stop)),
        ("signature", str_value(&row.signature)),
        ("message", str_value(&row.message)),
        (
            "pos",
            row.position
                .map(|(pos_x, pos_z, level)| Value::Arr(vec![Value::Int(pos_x), Value::Int(pos_z), Value::Int(level)]))
                .unwrap_or(Value::Null),
        ),
        ("zone", str_value(&row.zone)),
        ("detail", str_value(&row.detail)),
        ("dossier", str_value(&row.dossier)),
    ])
}

pub(crate) fn list_value(store: &Store, filter: &ErrorFilter) -> Result<Value, String> {
    let rows = store.errors_matching(filter).map_err(|error| error.to_string())?;
    let total = store.count_errors(filter).map_err(|error| error.to_string())?;
    Ok(object(vec![
        ("ok", Value::Bool(true)),
        ("total", Value::Int(total)),
        ("rows", Value::Arr(rows.iter().map(row_value).collect())),
    ]))
}

/// `<account>-<tick>.json`-like names only: no separator, no traversal.
pub(crate) fn dossier_name_ok(name: &str) -> bool {
    name.len() <= 128
        && name.ends_with(".json")
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

fn dossier_reply(run_dir: &Path, name: &str) -> Response {
    if !dossier_name_ok(name) {
        return ok_json(err_json(format!("bad dossier name '{name}'")));
    }
    match std::fs::read(run_dir.join("failures").join(name)) {
        Ok(body) => Response { status: 200, content_type: "application/json", cache: "no-store", body },
        Err(error) => Response {
            status: 404,
            content_type: "application/json",
            cache: "no-store",
            body: json::stringify(&err_json(format!("dossier {name}: {error}"))).into_bytes(),
        },
    }
}

/// `/api/errors/<op>` against a run dir (tests pass a temp one).
pub(crate) fn errors_api(parts: &[&str], query: &str, run_dir: &Path, now: i64) -> Response {
    if parts.len() == 2 && parts[0] == "dossier" {
        return dossier_reply(run_dir, parts[1]);
    }
    let (filter, hours) = match parse_filter(query, now) {
        Ok(parsed) => parsed,
        Err(error) => return ok_json(err_json(error)),
    };
    let store = match open_store(run_dir) {
        Ok(store) => store,
        Err(error) => return ok_json(err_json(error)),
    };
    let reply = match parts {
        ["summary"] => summary_value(&store, &filter, hours),
        ["list"] => list_value(&store, &filter),
        _ => Err(format!("unknown errors op '{}'", parts.join("/"))),
    };
    ok_json(reply.unwrap_or_else(err_json))
}

fn static_file(name: &str, content_type: &'static str) -> Response {
    match std::fs::read(crate::paths::static_dir().join(name)) {
        Ok(body) => Response { status: 200, content_type, cache: "no-store", body },
        Err(error) => Response {
            status: 500,
            content_type: "application/json",
            cache: "no-store",
            body: json::stringify(&err_json(error.to_string())).into_bytes(),
        },
    }
}

/// Router hook for `web::handle_get`: the page, its script and the API.
pub(crate) fn route(parts: &[&str], query: &str) -> Option<Response> {
    match parts {
        ["errors"] => Some(static_file("errors.html", "text/html; charset=utf-8")),
        ["errors.js"] => Some(static_file("errors.js", "application/javascript")),
        ["api", "errors", rest @ ..] => {
            let now = crate::jobs::now_secs() as i64;
            Some(errors_api(rest, query, &crate::paths::run_dir(), now))
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "web_errors_tests.rs"]
mod tests;
