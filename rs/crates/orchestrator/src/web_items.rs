//! Item holdings history endpoint (PRD 2, §6.5, D42, D48).
//!
//! Serves the frozen `HoldingsHistory` shape (`{ok,item,res,
//! points:[{t,v}]}`) over the store tables: `holdings_hourly` (8 days,
//! `res=1h`, the 24 h default) and `holdings_daily` (400 days, `res=1d`,
//! each value the last measure of its day). `item` is an obj id, an
//! exact item name, or absent for the whole-economy total.

#![forbid(unsafe_code)]

use std::path::Path;

use crate::api_shapes::{HoldingsHistory, HoldingsRes};
use crate::web::{err_json, ok_json, parse_query, Response};

fn store_for(run_dir: &Path) -> Result<lifescape_store::Store, String> {
    lifescape_store::Store::open(&run_dir.join("lifescape.db")).map_err(|error| format!("holdings store: {error}"))
}

/// Resolve the `item` query value: absent means the economy total
/// (`None`), digits mean an obj id, anything else an exact item name.
fn resolve_item(raw: Option<&str>) -> Result<(Option<i64>, String), String> {
    let Some(raw) = raw.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok((None, "total".to_string()));
    };
    if let Ok(obj_id) = raw.parse::<i64>() {
        if obj_id > 0 {
            return Ok((Some(obj_id), crate::holdings::item_name(obj_id)));
        }
        return Err(format!("bad item '{raw}'"));
    }
    let mut matches = lifescape_gamedata::objs_by_name(raw);
    matches.sort_by_key(|obj| obj.obj_id);
    match matches.as_slice() {
        [obj] => Ok((Some(obj.obj_id as i64), crate::holdings::item_name(obj.obj_id as i64))),
        [] => Err(format!("unknown item '{raw}'")),
        _ => Err(format!("ambiguous item '{raw}'")),
    }
}

fn history_points(store: &lifescape_store::Store, item: Option<i64>, res: HoldingsRes) -> Result<Vec<(i64, i64)>, String> {
    match res {
        HoldingsRes::Hour1 => store.hourly_points(item).map_err(|error| error.to_string()),
        HoldingsRes::Day1 => store.daily_points(item).map_err(|error| error.to_string()),
    }
}

/// `GET /api/items/history?res=1h|1d&item=`: the frozen history shape
/// for one item (or the total). Unknown `res` and unknown `item` read
/// loud; an empty history is a valid empty curve, not an error.
pub(crate) fn items_history_api(query: &str, run_dir: &Path) -> Response {
    let params = parse_query(query);
    let res = match params.get("res").map(String::as_str) {
        None => HoldingsRes::Hour1,
        Some(raw) => match HoldingsRes::parse(raw) {
            Some(res) => res,
            None => return ok_json(err_json(format!("bad res '{raw}': want 1h or 1d"))),
        },
    };
    let (item, label) = match resolve_item(params.get("item").map(String::as_str)) {
        Ok(resolved) => resolved,
        Err(error) => return ok_json(err_json(error)),
    };
    crate::holdings::ensure_imported(&run_dir.join("items"));
    let store = match store_for(run_dir) {
        Ok(store) => store,
        Err(error) => return ok_json(err_json(error)),
    };
    let points = match history_points(&store, item, res) {
        Ok(points) => points,
        Err(error) => return ok_json(err_json(error)),
    };
    ok_json(HoldingsHistory { item: label, res, points }.to_value())
}

/// `GET /api/items`: the live item list behind the history curves
/// (`{ok,liveTotal,items:[{id,name,count}]}`), ranked by count.
pub(crate) fn items_list_api(dispatch: &crate::web::Dispatch) -> Response {
    use lifescape_json::{object, str_value};
    let reply = dispatch(object(vec![("cmd", str_value("items"))]));
    let data = reply.get("data").clone();
    ok_json(if data.is_null() { reply } else { data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lifescape_json::{self as json, object, Value};

    /// The dashboard routes serve the frozen shapes end to end: the
    /// history curve with `res`/`item`, the live list beside it.
    #[test]
    fn routes_serve_history_and_list() {
        let root = std::env::temp_dir().join(format!("lifescape-items-routes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("run").join("items")).unwrap();
        let store = lifescape_store::Store::open(&root.join("run").join("lifescape.db")).unwrap();
        store.record_holdings(1_762_675_200, &[(995, 7)]).unwrap();
        let previous = std::env::var("LIFESCAPE_ROOT").ok();
        std::env::set_var("LIFESCAPE_ROOT", &root);
        let dispatch: crate::web::Dispatch = std::sync::Arc::new(|_| {
            object(vec![
                ("ok", Value::Bool(true)),
                ("data", object(vec![("liveTotal", Value::Int(7)), ("items", Value::Arr(vec![]))])),
            ])
        });
        let history = crate::web::handle_get("/api/items/history", "res=1d&item=995", &dispatch);
        assert_eq!(history.status, 200);
        let payload = json::parse(&String::from_utf8(history.body).expect("json body")).expect("json parses");
        assert_eq!(payload.get("ok").as_bool(), Some(true));
        assert_eq!(payload.get("res").as_str(), Some("1d"));
        assert_eq!(payload.get("item").as_str(), Some("Coins"));
        assert_eq!(payload.get("points").as_arr().map(Vec::len), Some(1));
        let listed = crate::web::handle_get("/api/items", "", &dispatch);
        let payload = json::parse(&String::from_utf8(listed.body).expect("json body")).expect("json parses");
        assert_eq!(payload.get("liveTotal").as_i64(), Some(7));
        match previous {
            Some(dir) => std::env::set_var("LIFESCAPE_ROOT", dir),
            None => std::env::remove_var("LIFESCAPE_ROOT"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    fn scratch_run(tag: &str) -> std::path::PathBuf {
        let run = std::env::temp_dir().join(format!("lifescape-items-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&run);
        std::fs::create_dir_all(run.join("items")).unwrap();
        run
    }

    fn body(reply: &Response) -> Value {
        json::parse(&String::from_utf8(reply.body.clone()).expect("json body")).expect("json parses")
    }

    /// One evening sample plus one hourly point the next noon: daily
    /// keeps the last measure of each day, hourly keeps the hours.
    #[test]
    fn history_serves_both_resolutions() {
        let run = scratch_run("history");
        let store = lifescape_store::Store::open(&run.join("lifescape.db")).unwrap();
        store.record_holdings(1_762_675_200, &[(995, 7)]).unwrap();
        store.record_holdings(1_762_675_200 + 5 * 3_600, &[(995, 11)]).unwrap();
        let hourly = body(&items_history_api("res=1h&item=995", &run));
        assert_eq!(hourly.get("ok").as_bool(), Some(true));
        assert_eq!(hourly.get("item").as_str(), Some("Coins"));
        assert_eq!(hourly.get("res").as_str(), Some("1h"));
        assert_eq!(hourly.get("points").as_arr().map(Vec::len), Some(2));
        let daily = body(&items_history_api("res=1d&item=995", &run));
        assert_eq!(daily.get("res").as_str(), Some("1d"));
        assert_eq!(daily.get("points").as_arr().map(Vec::len), Some(1), "one day, one point");
        assert_eq!(daily.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(11), "value is the last measure of the day");
        let _ = std::fs::remove_dir_all(&run);
    }

    #[test]
    fn history_defaults_rejects_and_names() {
        let run = scratch_run("res");
        let store = lifescape_store::Store::open(&run.join("lifescape.db")).unwrap();
        store.record_holdings(1_762_675_200, &[(995, 7)]).unwrap();
        // No res: the 24 h hourly view (D42).
        let plain = body(&items_history_api("item=995", &run));
        assert_eq!(plain.get("res").as_str(), Some("1h"));
        // Exact name resolves, total sums, unknown reads loud.
        let named = body(&items_history_api("res=1h&item=Coins", &run));
        assert_eq!(named.get("item").as_str(), Some("Coins"));
        assert_eq!(named.get("points").as_arr().map(Vec::len), Some(1));
        let total = body(&items_history_api("res=1h", &run));
        assert_eq!(total.get("item").as_str(), Some("total"));
        assert_eq!(total.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(7));
        assert_eq!(body(&items_history_api("res=30m&item=995", &run)).get("ok").as_bool(), Some(false));
        assert_eq!(body(&items_history_api("res=1h&item=nope-not-an-item", &run)).get("ok").as_bool(), Some(false));
        assert_eq!(body(&items_history_api("res=1h&item=0", &run)).get("ok").as_bool(), Some(false));
        // Empty store: valid empty curve, never an error.
        let bare = scratch_run("empty");
        let empty = body(&items_history_api("res=1d", &bare));
        assert_eq!(empty.get("ok").as_bool(), Some(true));
        assert_eq!(empty.get("points").as_arr().map(Vec::len), Some(0));
        let _ = std::fs::remove_dir_all(&run);
        let _ = std::fs::remove_dir_all(&bare);
    }

    /// The frozen JSON history feeds the endpoint through the one-shot
    /// import, then steps aside.
    #[test]
    fn history_imports_the_frozen_json_once() {
        let run = scratch_run("import");
        std::fs::write(
            run.join("items").join("holdings.json"),
            r#"{"samples": [{"time": 1000, "items": {"995": 7}}, {"time": 5000, "items": {"995": 5}}]}"#,
        )
        .unwrap();
        let daily = body(&items_history_api("res=1d&item=995", &run));
        assert_eq!(daily.get("points").as_arr().map(Vec::len), Some(1), "same day: one point");
        assert_eq!(daily.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(5), "last measure of the day wins");
        assert!(!run.join("items").join("holdings.json").exists());
        assert!(run.join("items").join("holdings.json.imported").exists());
        let _ = std::fs::remove_dir_all(&run);
    }
}
