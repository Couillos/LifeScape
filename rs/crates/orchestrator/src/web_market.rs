#![forbid(unsafe_code)]
//! Market routes of the dashboard (split from web.rs, same behavior):
//! history window snapping plus the `/api/market/<op>` bridge used by handle_get.

use lifescape_json::{Value, str_value};
use crate::web::{Dispatch, Response, ok_json, parse_query};

/// Candle width of a history resolution: `1d` days, anything else hours
/// (same mapping as the market `history` op).
fn res_width(res: &str) -> i64 {
    if res == "1d" { 86400 } else { 3600 }
}

/// Snap a requested history window to candle boundaries: `from` floors to
/// the resolution, `to` ceils so the live candle stays inside the
/// half-open `[from, to)` the store reads. At least one width apart.
pub fn snap_range(from: i64, to: i64, res: &str) -> (i64, i64) {
    let width = res_width(res);
    let snapped_from = (from.max(0) / width) * width;
    let snapped_to = ((to.max(1) + width - 1) / width) * width;
    (snapped_from, snapped_to.max(snapped_from + width))
}

/// Market JSON API bridge for handle_get: the op is the path segment
/// after `/api/market/`, the rest of the query string becomes fields.
pub(crate) fn market_api(op: &str, query: &str, dispatch: &Dispatch) -> Response {
        // Market JSON API: the op is the third segment, the rest of
        // the query string becomes the request fields.
        let params = parse_query(query);
        let num = |key: &str| params.get(key).and_then(|raw| raw.parse::<i64>().ok());
        let mut fields: Vec<(String, Value)> = vec![("cmd".to_string(), str_value("market")), ("op".to_string(), str_value(&op))];
        for key in ["item", "depth", "from", "to", "limit", "since"] {
            if let Some(value) = num(key) {
                fields.push((key.to_string(), Value::Int(value)));
            }
        }
        for key in ["q", "res", "account", "side"] {
            if let Some(value) = params.get(key) {
                // Search names carry spaces (`Air rune`): decode the
                // two escapes browsers send, nothing more.
                let decoded = value.replace('+', " ").replace("%20", " ");
                fields.push((key.to_string(), str_value(&decoded)));
            }
        }
        // History reads half-open `[from, to)` candle windows: snap
        // both bounds to the resolution so the live candle stays
        // inside, whatever the client clock sent.
        if op == "history" {
            if let (Some(from), Some(to)) = (num("from"), num("to")) {
                let res = params.get("res").map(String::as_str).unwrap_or("1h");
                let (snapped_from, snapped_to) = snap_range(from, to, res);
                fields.retain(|(key, _)| key != "from" && key != "to");
                fields.push(("from".to_string(), Value::Int(snapped_from)));
                fields.push(("to".to_string(), Value::Int(snapped_to)));
            }
        }
        let mut map = std::collections::BTreeMap::new();
        for (key, value) in fields {
            map.insert(key, value);
        }
ok_json(dispatch(Value::Obj(map)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use lifescape_json::{self as json, object};

    #[test]
    fn history_window_snaps_to_candle_edges() {
        // Day resolution: from floors, to ceils so the live candle stays in.
        assert_eq!(snap_range(100001, 200001, "1d"), (86400, 259200));
        // Hour resolution, aligned bounds pass through.
        assert_eq!(snap_range(3600, 7200, "1h"), (3600, 7200));
        assert_eq!(snap_range(3601, 7200, "1h"), (3600, 7200));
        assert_eq!(snap_range(3600, 7201, "1h"), (3600, 10800));
        // Unknown resolutions read hourly, like the market op.
        assert_eq!(snap_range(3601, 7201, "bogus"), (3600, 10800));
        // Degenerate spans keep at least one candle.
        assert_eq!(snap_range(5000, 5000, "1h"), (3600, 7200));
    }

    #[test]
    fn market_history_forwards_the_snapped_window() {
        fn echo_dispatch(request: Value) -> Value {
            object(vec![
                ("ok", Value::Bool(true)),
                ("op", request.get("op").clone()),
                ("from", request.get("from").clone()),
                ("to", request.get("to").clone()),
                ("res", request.get("res").clone()),
            ])
        }
        let dispatch: Dispatch = Arc::new(echo_dispatch);
        let reply = crate::web::handle_get("/api/market/history", "item=7&res=1d&from=100001&to=200001", &dispatch);
        assert_eq!(reply.status, 200);
        let body = String::from_utf8(reply.body).expect("json body");
        let payload = json::parse(&body).expect("json parses");
        assert_eq!(payload.get("from").as_i64(), Some(86400));
        assert_eq!(payload.get("to").as_i64(), Some(259200));
        assert_eq!(payload.get("res").as_str(), Some("1d"));
        // Other ops pass their bounds through untouched.
        let reply = crate::web::handle_get("/api/market/trades", "item=7&limit=5", &dispatch);
        let payload = json::parse(&String::from_utf8(reply.body).expect("json body")).expect("json parses");
        assert_eq!(payload.get("op").as_str(), Some("trades"));
        assert!(payload.get("from").is_null());
    }
}
