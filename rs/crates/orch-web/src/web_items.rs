//! Item holdings history endpoint (PRD 2, §6.5, D42, D48).
//!
//! Serves the frozen `HoldingsHistory` shape (`{ok,item,res,
//! points:[{t,v}]}`) over the store tables: `holdings_hourly` (8 days,
//! `res=1h`, the 24 h default) and `holdings_daily` (400 days, `res=1d`,
//! each value the last measure of its day). `item` is an obj id, an
//! exact item name, or absent for the whole-economy total; `index` a
//! supply index key (`holdings_indices`), its objects summed per slot.
//! `/api/items/flows` serves an object's sources and sinks by provenance
//! (task report lines, `Store::item_flows_since`), staff excluded.

#![forbid(unsafe_code)]

use std::path::Path;

use crate::api_shapes::{HoldingsHistory, HoldingsRes};
use crate::web::{err_json, ok_json, parse_query, Response};

fn store_for(run_dir: &Path) -> Result<crate::store_pool::PooledStore, String> {
    crate::store_pool::open(&run_dir.join("lifescape.db")).map_err(|error| format!("holdings store: {error}"))
}

/// Resolve the `item` query value: absent means the economy total
/// (`None`), digits mean an obj id, anything else an exact item name.
fn resolve_item(raw: Option<&str>) -> Result<(Option<i64>, String), String> {
    let Some(raw) = raw.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok((None, "total".to_string()));
    };
    // A banknote names its object (user 2026-11-19): one curve.
    let base = |obj_id: usize| lifescape_gamedata::base_obj_id(obj_id) as i64;
    if let Ok(obj_id) = raw.parse::<i64>() {
        if obj_id > 0 {
            let object = usize::try_from(obj_id).map(base).unwrap_or(obj_id);
            return Ok((Some(object), crate::holdings::item_name(object)));
        }
        return Err(format!("bad item '{raw}'"));
    }
    let mut matches = lifescape_gamedata::objs_by_name(raw);
    matches.sort_by_key(|obj| obj.obj_id);
    match matches.as_slice() {
        [obj] => Ok((Some(base(obj.obj_id)), crate::holdings::item_name(base(obj.obj_id)))),
        [] => Err(format!("unknown item '{raw}'")),
        _ => Err(format!("ambiguous item '{raw}'")),
    }
}

/// What one history curve sums: one object, the whole economy, or a
/// supply index (its objects and their notes).
enum Curve {
    Item(Option<i64>),
    Index(Vec<i64>),
}

fn history_points(store: &lifescape_store::Store, curve: &Curve, res: HoldingsRes) -> Result<Vec<(i64, i64)>, String> {
    let points = match (curve, res) {
        // One object, all its forms: rows recorded before the notes were
        // folded (2026-11-19) still sit under the note's own id.
        (Curve::Item(Some(item)), HoldingsRes::Hour1) => store.hourly_points_sum(&item_forms(*item)),
        (Curve::Item(Some(item)), HoldingsRes::Day1) => store.daily_points_sum(&item_forms(*item)),
        // The item total leaves the coins out (user 2026-11-19).
        (Curve::Item(None), HoldingsRes::Hour1) => store.hourly_points_without(&[crate::holdings::COINS]),
        (Curve::Item(None), HoldingsRes::Day1) => store.daily_points_without(&[crate::holdings::COINS]),
        (Curve::Index(ids), HoldingsRes::Hour1) => store.hourly_points_sum(ids),
        (Curve::Index(ids), HoldingsRes::Day1) => store.daily_points_sum(ids),
    };
    points.map_err(|error| error.to_string())
}

/// Every id `item` is held as (the object, its banknote).
fn item_forms(item: i64) -> Vec<i64> {
    usize::try_from(item).map(|id| lifescape_gamedata::obj_forms(id).into_iter().map(|form| form as i64).collect()).unwrap_or_else(|_| vec![item])
}

/// The curve a query asks for: `index=<key>` (a supply index, alone) or
/// `item=` (see `resolve_item`).
fn resolve_curve(params: &std::collections::HashMap<String, String>) -> Result<(Curve, String), String> {
    let Some(key) = params.get("index").map(|raw| raw.trim()).filter(|key| !key.is_empty()) else {
        let (item, label) = resolve_item(params.get("item").map(String::as_str))?;
        return Ok((Curve::Item(item), label));
    };
    if params.get("item").is_some_and(|item| !item.trim().is_empty()) {
        return Err("item and index are exclusive".to_string());
    }
    let index = lifescape_gamedata::supply_indices::SupplyIndex::parse(key).ok_or_else(|| format!("unknown index '{key}'"))?;
    Ok((Curve::Index(crate::holdings_indices::index_ids(index)), index.label().to_string()))
}

/// `GET /api/items/history?res=1h|1d&item=|index=`: the frozen history
/// shape for one item, the total, or a supply index (`item` then carries
/// the index label). Unknown `res`, `item` or `index` read loud; an
/// empty history is a valid empty curve, not an error.
pub(crate) fn items_history_api(query: &str, run_dir: &Path) -> Response {
    let params = parse_query(query);
    let res = match params.get("res").map(String::as_str) {
        None => HoldingsRes::Hour1,
        Some(raw) => match HoldingsRes::parse(raw) {
            Some(res) => res,
            None => return ok_json(err_json(format!("bad res '{raw}': want 1h or 1d"))),
        },
    };
    let (curve, label) = match resolve_curve(&params) {
        Ok(resolved) => resolved,
        Err(error) => return ok_json(err_json(error)),
    };
    crate::holdings::ensure_imported(&run_dir.join("items"));
    let store = match store_for(run_dir) {
        Ok(store) => store,
        Err(error) => return ok_json(err_json(error)),
    };
    let points = match history_points(&store, &curve, res) {
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

/// Rows served for the all-items flow view (same cut as the CLI).
const FLOW_ROWS_ALL: usize = 60;
/// Longest flow window: the 6-month chart period plus slack.
const FLOW_MAX_HOURS: i64 = 400 * 24;

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0)
}

/// Resolve the flows `item` value to the display name the task reports
/// carry: absent means every item, digits an obj id, anything else a
/// (percent-decoded) display name matched case-insensitively.
fn flows_item(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw.map(crate::web_errors::percent_decode) else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(obj_id) = trimmed.parse::<i64>() {
        if obj_id > 0 {
            return Ok(Some(crate::holdings::item_name(obj_id)));
        }
        return Err(format!("bad item '{trimmed}'"));
    }
    Ok(Some(trimmed.to_string()))
}

/// The flows payload over the store: `{ok,item,hours,since,flows:[{item,
/// provenance,quantity,lines}]}`, sorted by |quantity| (store order),
/// cut to `FLOW_ROWS_ALL` rows without an item filter.
fn flows_payload(store: &lifescape_store::Store, item: Option<&str>, hours: i64, since: i64, excluded: &[String]) -> Result<lifescape_json::Value, String> {
    use lifescape_json::{object, str_value, Value};
    let mut flows = store.item_flows_since(since, excluded, item).map_err(|error| format!("item flows: {error}"))?;
    if item.is_none() {
        flows.truncate(FLOW_ROWS_ALL);
    }
    let rows = flows
        .iter()
        .map(|(name, provenance, quantity, lines)| {
            object(vec![
                ("item", str_value(name)),
                ("provenance", str_value(provenance)),
                ("quantity", Value::Int(*quantity)),
                ("lines", Value::Int(*lines)),
            ])
        })
        .collect();
    Ok(object(vec![
        ("ok", Value::Bool(true)),
        ("item", item.map(str_value).unwrap_or(Value::Null)),
        ("hours", Value::Int(hours)),
        ("since", Value::Int(since)),
        ("flows", Value::Arr(rows)),
    ]))
}

/// `GET /api/items/flows?item=&hours=24`: where one object (obj id or
/// display name; absent = the main flows of every object) came from and
/// went over the last `hours`, by provenance (task report lines, + =
/// entered a pack, - = left it), staff and persisted exclusions out.
/// Same data as `orchestrator economy [hours] item <name>`.
pub(crate) fn items_flows_api(query: &str, run_dir: &Path) -> Response {
    let params = parse_query(query);
    let hours = match params.get("hours").map(String::as_str) {
        None => 24,
        Some(raw) => match raw.parse::<i64>() {
            Ok(hours) if (1..=FLOW_MAX_HOURS).contains(&hours) => hours,
            _ => return ok_json(err_json(format!("bad hours '{raw}': want 1..{FLOW_MAX_HOURS}"))),
        },
    };
    let item = match flows_item(params.get("item").map(String::as_str)) {
        Ok(item) => item,
        Err(error) => return ok_json(err_json(error)),
    };
    let store = match store_for(run_dir) {
        Ok(store) => store,
        Err(error) => return ok_json(err_json(error)),
    };
    let excluded: Vec<String> = crate::economy_guard::excluded_accounts().into_iter().collect();
    match flows_payload(&store, item.as_deref(), hours, now_secs() - hours * 3600, &excluded) {
        Ok(payload) => ok_json(payload),
        Err(error) => ok_json(err_json(error)),
    }
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
        store.record_holdings(1_762_675_200, &[(995, 7), (1515, 3)]).unwrap();
        // No res: the 24 h hourly view (D42).
        let plain = body(&items_history_api("item=995", &run));
        assert_eq!(plain.get("res").as_str(), Some("1h"));
        // Exact name resolves, total sums, unknown reads loud.
        let named = body(&items_history_api("res=1h&item=Coins", &run));
        assert_eq!(named.get("item").as_str(), Some("Coins"));
        assert_eq!(named.get("points").as_arr().map(Vec::len), Some(1));
        // The item total leaves the coins out (user 2026-11-19).
        let total = body(&items_history_api("res=1h", &run));
        assert_eq!(total.get("item").as_str(), Some("total"));
        assert_eq!(total.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(3), "3 yew logs, never the 7 coins");
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

    /// A supply index sums its objects (and their notes) per slot and
    /// names itself; an unknown index or one mixed with an item reads loud.
    /// A note is its object (user 2026-11-19): asked by note or by base,
    /// the curve is the object's, its old note rows summed in.
    #[test]
    fn an_item_curve_sums_its_notes() {
        let run = scratch_run("notes");
        let store = super::store_for(&run).unwrap();
        store.record_holdings(1_762_675_200, &[(1515, 100), (1516, 28)]).unwrap();
        drop(store);
        for item in ["1515", "1516", "yew_logs", "cert_yew_logs"] {
            let reply = super::items_history_api(&format!("res=1h&item={item}"), &run);
            let value = body(&reply);
            assert_eq!(value.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(128), "{item}: {value:?}");
        }
    }

    #[test]
    fn history_serves_supply_indices() {
        let run = scratch_run("index");
        let store = lifescape_store::Store::open(&run.join("lifescape.db")).unwrap();
        let id_of = |name: &str| lifescape_gamedata::objs_by_name(name)[0].obj_id as i64;
        store.record_holdings(1_762_675_200, &[(id_of("raw_shrimp"), 4), (id_of("cert_raw_shrimp"), 6), (id_of("shrimp"), 9), (995, 50)]).unwrap();
        let cooking = body(&items_history_api("res=1h&index=cooking", &run));
        assert_eq!(cooking.get("ok").as_bool(), Some(true));
        assert_eq!(cooking.get("item").as_str(), Some("Cooking"));
        assert_eq!(cooking.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(10), "raw shrimps and their notes");
        let food = body(&items_history_api("res=1d&index=food", &run));
        assert_eq!(food.get("res").as_str(), Some("1d"));
        assert_eq!(food.get("points").as_arr().unwrap()[0].get("v").as_i64(), Some(9));
        let tools = body(&items_history_api("res=1h&index=tools", &run));
        assert_eq!(tools.get("points").as_arr().map(Vec::len), Some(0), "nothing held: empty curve");
        assert_eq!(body(&items_history_api("res=1h&index=nope", &run)).get("ok").as_bool(), Some(false));
        assert_eq!(body(&items_history_api("res=1h&index=food&item=995", &run)).get("ok").as_bool(), Some(false));
        let _ = std::fs::remove_dir_all(&run);
    }

    fn report_with(store: &lifescape_store::Store, account: &str, clock: i64, lines: &[(&str, &str, i64)]) {
        let report_id = store
            .record_task_report(&lifescape_store::NewTaskReport {
                account: account.to_string(),
                node_id: "node".to_string(),
                method: "method".to_string(),
                script: "script".to_string(),
                ticks: 10,
                stop: "done".to_string(),
                context_level: 1,
                clock,
            })
            .unwrap();
        for (item_name, provenance, quantity) in lines {
            store
                .record_task_yield(&lifescape_store::NewTaskYield {
                    report_id,
                    item_name: item_name.to_string(),
                    provenance: provenance.to_string(),
                    quantity: *quantity,
                    skill: String::new(),
                    xp: 0,
                })
                .unwrap();
        }
    }

    /// One object's flows by provenance over the window: lines summed
    /// across accounts, staff and older reports left out, sorted by size;
    /// without a filter every object shows.
    #[test]
    fn flows_payload_sums_filters_and_excludes() {
        let store = lifescape_store::Store::open_in_memory().unwrap();
        report_with(&store, "bot-one", 5_000, &[("Logs", "produced", 12), ("Logs", "shop_sold", -10), ("Coins", "shop_sold", 40)]);
        report_with(&store, "bot-two", 6_000, &[("Logs", "produced", 3)]);
        report_with(&store, "admin", 6_000, &[("Logs", "loot", 999)]);
        report_with(&store, "bot-one", 1_000, &[("Logs", "lost", -50)]);
        let excluded = vec!["admin".to_string()];
        let logs = flows_payload(&store, Some("logs"), 1, 4_000, &excluded).unwrap();
        assert_eq!(logs.get("ok").as_bool(), Some(true));
        assert_eq!(logs.get("item").as_str(), Some("logs"));
        assert_eq!(logs.get("hours").as_i64(), Some(1));
        assert_eq!(logs.get("since").as_i64(), Some(4_000));
        let rows = logs.get("flows").as_arr().unwrap();
        assert_eq!(rows.len(), 2, "produced + shop_sold; admin loot and the old loss are out");
        assert_eq!(rows[0].get("provenance").as_str(), Some("produced"));
        assert_eq!(rows[0].get("quantity").as_i64(), Some(15));
        assert_eq!(rows[0].get("lines").as_i64(), Some(2));
        assert_eq!(rows[1].get("provenance").as_str(), Some("shop_sold"));
        assert_eq!(rows[1].get("quantity").as_i64(), Some(-10));
        let every = flows_payload(&store, None, 1, 4_000, &excluded).unwrap();
        assert!(every.get("item").is_null());
        let names: Vec<&str> = every.get("flows").as_arr().unwrap().iter().filter_map(|row| row.get("item").as_str()).collect();
        assert_eq!(names, vec!["Coins", "Logs", "Logs"]);
    }

    /// The route reads loud on a bad window or item, serves an empty
    /// list on an empty store, and resolves an obj id to its name.
    #[test]
    fn flows_route_validates_and_resolves() {
        let run = scratch_run("flows");
        let store = lifescape_store::Store::open(&run.join("lifescape.db")).unwrap();
        report_with(&store, "bot-one", now_secs() - 60, &[("Coins", "shop_sold", 40)]);
        let coins = body(&items_flows_api("item=995&hours=24", &run));
        assert_eq!(coins.get("ok").as_bool(), Some(true));
        assert_eq!(coins.get("item").as_str(), Some("Coins"));
        assert_eq!(coins.get("flows").as_arr().map(Vec::len), Some(1));
        let named = body(&items_flows_api("item=coins", &run));
        assert_eq!(named.get("hours").as_i64(), Some(24), "default window");
        assert_eq!(named.get("flows").as_arr().map(Vec::len), Some(1));
        let spaced = body(&items_flows_api("item=Raw%20shrimps&hours=168", &run));
        assert_eq!(spaced.get("item").as_str(), Some("Raw shrimps"));
        assert_eq!(spaced.get("flows").as_arr().map(Vec::len), Some(0), "nothing recorded is an empty list");
        assert_eq!(body(&items_flows_api("hours=0", &run)).get("ok").as_bool(), Some(false));
        assert_eq!(body(&items_flows_api("hours=abc", &run)).get("ok").as_bool(), Some(false));
        assert_eq!(body(&items_flows_api("item=0", &run)).get("ok").as_bool(), Some(false));
        // Routed by the dashboard: a bad window answers the route's own
        // error (before any store is opened), never the 404 fallback.
        let dispatch: crate::web::Dispatch = std::sync::Arc::new(|_| Value::Null);
        let routed = crate::web::handle_get("/api/items/flows", "hours=0", &dispatch);
        assert_eq!(routed.status, 200);
        assert!(body(&routed).get("error").as_str().unwrap_or("").starts_with("bad hours"));
        let _ = std::fs::remove_dir_all(&run);
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
