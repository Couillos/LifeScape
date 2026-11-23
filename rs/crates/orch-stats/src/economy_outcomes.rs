#![forbid(unsafe_code)]
//! Economy page: cancelled / failed transactions and their percentage,
//! over 1 h, 24 h, 7 days and the whole history.
//!
//! Exchanges (deals between two bots): settled, failed (by reason), in
//! progress; failed % = failed / (settled + failed).
//! Offers (orders): one per real order (a price change is not a new
//! order, see `lifescape_store::outcomes`): settled, cancelled (by reason),
//! open; cancelled % = cancelled / (settled + cancelled).

use lifescape_json::{Value, object, str_value};
use lifescape_store::outcomes::OutcomeCount;

/// (key, label, window length in seconds; 0 = whole history).
pub const WINDOWS: [(&str, &str, i64); 4] =
    [("1h", "1 h", 3600), ("24h", "24 h", 86_400), ("7d", "7 jours", 604_800), ("all", "tout l'historique", 0)];

/// Lower clock bound of each window at `now`.
pub fn since_bounds(now: i64) -> Vec<i64> {
    WINDOWS.iter().map(|(_, _, seconds)| if *seconds == 0 { 0 } else { now - seconds }).collect()
}

/// Percentage 0..100 of `part` among `part + other`; `None` when empty.
fn percent(part: i64, other: i64) -> Option<f64> {
    let total = part + other;
    (total > 0).then(|| part as f64 * 100.0 / total as f64)
}

/// Sums one window of the rows matching `statuses`.
fn sum_statuses(rows: &[OutcomeCount], window: usize, statuses: &[&str]) -> i64 {
    rows.iter().filter(|row| statuses.contains(&row.status.as_str())).map(|row| row.per_window[window]).sum()
}

/// Reasons of one terminal status, biggest first, zero counts dropped.
fn reasons(rows: &[OutcomeCount], window: usize, status: &str) -> Value {
    let mut found: Vec<(&str, i64)> = rows
        .iter()
        .filter(|row| row.status == status && row.per_window[window] > 0)
        .map(|row| (row.reason.as_str(), row.per_window[window]))
        .collect();
    found.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    Value::Arr(
        found
            .into_iter()
            .map(|(reason, count)| object(vec![("reason", str_value(reason)), ("count", Value::Int(count))]))
            .collect(),
    )
}

/// One window of one kind: `lost_status` is the failure status
/// (`failed` for deals, `cancelled` for orders), `open_statuses` the
/// not-yet-final ones.
fn window_value(rows: &[OutcomeCount], window: usize, lost_status: &str, open_statuses: &[&str]) -> Value {
    let (key, label, _) = WINDOWS[window];
    let settled = sum_statuses(rows, window, &["settled"]);
    let lost = sum_statuses(rows, window, &[lost_status]);
    let open = sum_statuses(rows, window, open_statuses);
    object(vec![
        ("key", str_value(key)),
        ("label", str_value(label)),
        ("settled", Value::Int(settled)),
        ("lost", Value::Int(lost)),
        ("open", Value::Int(open)),
        ("lost_percent", percent(lost, settled).map_or(Value::Null, Value::Float)),
        ("reasons", reasons(rows, window, lost_status)),
    ])
}

fn kind_value(rows: &[OutcomeCount], lost_status: &str, open_statuses: &[&str]) -> Value {
    Value::Arr((0..WINDOWS.len()).map(|window| window_value(rows, window, lost_status, open_statuses)).collect())
}

/// Payload block `outcomes`: `deals` and `orders`, one entry per window.
pub fn outcomes_value(deal_rows: &[OutcomeCount], order_rows: &[OutcomeCount]) -> Value {
    object(vec![
        ("deals", kind_value(deal_rows, "failed", &["matched", "convened", "trading"])),
        ("orders", kind_value(order_rows, "cancelled", &["open", "matched"])),
    ])
}

/// Seconds a computed block is reused: the order scan reads the whole
/// table (about 0.7 s CPU on 1.4 M rows), the page polls every 30 s.
const CACHE_SECONDS: i64 = 60;

static CACHE: std::sync::Mutex<Option<(std::path::PathBuf, i64, Value)>> = std::sync::Mutex::new(None);

/// `outcomes_value` over the store, cached for `CACHE_SECONDS` per store file.
pub fn cached_outcomes(
    key: &std::path::Path,
    now: i64,
    compute: impl FnOnce() -> Result<Value, lifescape_store::Error>,
) -> Result<Value, lifescape_store::Error> {
    if let Ok(guard) = CACHE.lock() {
        if let Some((cached_key, at, value)) = guard.as_ref() {
            if cached_key == key && now - at < CACHE_SECONDS && now >= *at {
                return Ok(value.clone());
            }
        }
    }
    let value = compute()?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((key.to_path_buf(), now, value.clone()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, reason: &str, counts: [i64; 4]) -> OutcomeCount {
        OutcomeCount { status: status.to_string(), reason: reason.to_string(), per_window: counts.to_vec() }
    }

    #[test]
    fn deals_percentage_is_failed_over_settled_plus_failed() {
        let rows = vec![
            row("settled", "", [6, 6, 60, 600]),
            row("failed", "timeout", [1, 2, 20, 100]),
            row("failed", "no_show", [1, 2, 20, 200]),
            row("matched", "", [3, 3, 3, 3]),
        ];
        let value = outcomes_value(&rows, &[]);
        let windows = value.get("deals").as_arr().expect("deal windows");
        assert_eq!(windows[0].get("settled").as_i64(), Some(6));
        assert_eq!(windows[0].get("lost").as_i64(), Some(2));
        assert_eq!(windows[0].get("open").as_i64(), Some(3), "in progress is neither settled nor failed");
        assert_eq!(windows[0].get("lost_percent").as_f64(), Some(25.0), "2 / (6 + 2), open excluded");
        let reasons = windows[3].get("reasons").as_arr().expect("reasons");
        assert_eq!(reasons[0].get("reason").as_str(), Some("no_show"), "biggest reason first");
    }

    #[test]
    fn orders_percentage_and_empty_window() {
        let rows = vec![
            row("settled", "", [0, 1, 1, 1]),
            row("cancelled", "need_met", [0, 3, 3, 3]),
            row("open", "", [4, 4, 4, 4]),
        ];
        let value = outcomes_value(&[], &rows);
        let windows = value.get("orders").as_arr().expect("order windows");
        assert!(windows[0].get("lost_percent").is_null(), "nothing final in the hour: no percentage");
        assert_eq!(windows[0].get("open").as_i64(), Some(4));
        assert_eq!(windows[1].get("lost_percent").as_f64(), Some(75.0));
    }

    #[test]
    fn cache_reuses_within_a_minute_then_recomputes() {
        let key = std::path::Path::new("/tmp/cache-test-key");
        let first = cached_outcomes(key, 1000, || Ok(Value::Int(1))).expect("first");
        let reused = cached_outcomes(key, 1030, || Ok(Value::Int(2))).expect("reused");
        let fresh = cached_outcomes(key, 1061, || Ok(Value::Int(3))).expect("fresh");
        assert_eq!((first.as_i64(), reused.as_i64(), fresh.as_i64()), (Some(1), Some(1), Some(3)));
    }

    #[test]
    fn bounds_leave_the_whole_history_open() {
        assert_eq!(since_bounds(100_000), vec![96_400, 13_600, -504_800, 0]);
    }
}
