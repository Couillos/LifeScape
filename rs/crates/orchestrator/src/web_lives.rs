#![forbid(unsafe_code)]
//! Lives + economy domain of the dashboard (split from web.rs, same
//! behavior): life files verbatim, the life index and the economy payload.

use std::time::{SystemTime, UNIX_EPOCH};

use lifescape_json::{self as json, Value, object, str_value};
use crate::web::{Dispatch, err_json};

fn lives_dir(run_dir: &std::path::Path) -> std::path::PathBuf {
    run_dir.join("lives")
}

/// One life file verbatim: goal, nodes (status, method, alternatives with
/// costs), engagement, history. Manual bots have no file: loud error.
pub fn life_response(account: &str) -> Value {
    life_response_in(&crate::paths::run_dir(), account)
}

/// One life file verbatim (see `life_response`); `run_dir` is explicit so
/// tests serve a scratch root without touching the process environment.
pub fn life_response_in(run_dir: &std::path::Path, account: &str) -> Value {
    if account.is_empty() || account == "." || account == ".." || account.contains('/') {
        return err_json(format!("bad account '{account}'"));
    }
    let text = match std::fs::read_to_string(lives_dir(run_dir).join(format!("{account}.json"))) {
        Ok(text) => text,
        Err(_) => return err_json(format!("no life for {account}")),
    };
    match json::parse(&text) {
        Ok(life) => object(vec![("ok", Value::Bool(true)), ("life", life)]),
        Err(error) => err_json(format!("life {account} corrupt: {error}")),
    }
}

/// Every account with a life file: goal plus node counts by status.
pub fn lives_response() -> Value {
    lives_response_in(&crate::paths::run_dir())
}

/// Life index (see `lives_response`) over an explicit root.
pub fn lives_response_in(run_dir: &std::path::Path) -> Value {
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(lives_dir(run_dir)) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let account = name[..name.len() - 5].to_string();
            let Ok(text) = std::fs::read_to_string(lives_dir(run_dir).join(&name)) else {
                continue;
            };
            let Ok(life) = json::parse(&text) else {
                continue;
            };
            let mut counts = std::collections::BTreeMap::new();
            for node in life.get("nodes").as_arr().cloned().unwrap_or_default() {
                let status = node.get("status").as_str().unwrap_or("pending").to_string();
                *counts.entry(status).or_insert(0i64) += 1;
            }
            let mut summary = std::collections::BTreeMap::new();
            for (status, count) in counts {
                summary.insert(status, Value::Int(count));
            }
            rows.push(object(vec![
                ("account", str_value(&account)),
                ("goal", life.get("goal").clone()),
                ("active", life.get("active").clone()),
                ("ironman", life.get("profile").get("ironman").clone()),
                ("nodes", Value::Obj(summary)),
            ]));
        }
    }
    object(vec![("ok", Value::Bool(true)), ("lives", Value::Arr(rows))])
}

/// Open sell/buy exposure of one merchant account from its open orders.
fn merchant_exposure(dispatch: &Dispatch, account: &str) -> (i64, i64, i64, usize, Vec<i64>) {
    let orders = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("my_orders")),
        ("account", str_value(account)),
    ]));
    let mut buy_value = 0i64;
    let mut sell_qty = 0i64;
    let mut sell_value = 0i64;
    let mut open = 0usize;
    let mut items = Vec::new();
    for row in orders.get("orders").as_arr().cloned().unwrap_or_default() {
        if row.get("status").as_str() != Some("open") {
            continue;
        }
        open += 1;
        let left = row.get("left").as_i64().unwrap_or(0).max(0);
        let price = row.get("price").as_i64().unwrap_or(0).max(0);
        let item = row.get("item").as_i64().unwrap_or(-1);
        if row.get("side").as_str() == Some("buy") {
            buy_value = buy_value.saturating_add(left.saturating_mul(price));
        } else {
            sell_qty = sell_qty.saturating_add(left);
            sell_value = sell_value.saturating_add(left.saturating_mul(price));
        }
        if item >= 0 && !items.contains(&item) {
            items.push(item);
        }
    }
    (buy_value, sell_qty, sell_value, open, items)
}

/// Depth quantity of one book side: `[[price, qty], …]`.
fn book_side_qty(book: &Value, side: &str) -> i64 {
    book.get(side)
        .as_arr()
        .map(|levels| {
            levels
                .iter()
                .map(|level| level.as_arr().and_then(|cells| cells.get(1)).and_then(|qty| qty.as_i64()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// Largest share of resting depth held by open orders on one side: the
/// merchant's weight in the book on the items they quote.
fn merchant_book_share(dispatch: &Dispatch, account: &str, items: &[i64]) -> f64 {
    let orders = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("my_orders")),
        ("account", str_value(account)),
    ]));
    let mut open: std::collections::BTreeMap<(i64, String), i64> = std::collections::BTreeMap::new();
    for row in orders.get("orders").as_arr().cloned().unwrap_or_default() {
        if row.get("status").as_str() != Some("open") {
            continue;
        }
        let key = (row.get("item").as_i64().unwrap_or(-1), row.get("side").as_str().unwrap_or("").to_string());
        *open.entry(key).or_insert(0) += row.get("left").as_i64().unwrap_or(0).max(0);
    }
    let mut top = 0.0f64;
    for item in items.iter().take(12) {
        let book = dispatch(object(vec![
            ("cmd", str_value("market")),
            ("op", str_value("book")),
            ("item", Value::Int(*item)),
            ("depth", Value::Int(10)),
        ]));
        for (side, key) in [("buys", "buy"), ("sells", "sell")] {
            let depth = book_side_qty(&book, side);
            let held = open.get(&(*item, key.to_string())).cloned().unwrap_or(0) as f64;
            if held > 0.0 {
                let share = if depth > 0 { held / depth as f64 } else { 1.0 };
                if share > top {
                    top = share;
                }
            }
        }
    }
    top
}

/// Economy payload from existing sources only: holdings via the `items`
/// command, market stats via the `market` command, deals/trends/merchants
/// via the store, top activities via the rates table. One JSON fetch for
/// the economy page; bots never read it.
pub fn economy_response(dispatch: &Dispatch) -> Value {
    economy_response_in(dispatch, &crate::paths::run_dir())
}

/// Economy payload (see `economy_response`) over an explicit root.
pub fn economy_response_in(dispatch: &Dispatch, run_dir: &std::path::Path) -> Value {
    let store = match lifescape_store::Store::open(&run_dir.join("lifescape.db")) {
        Ok(store) => store,
        Err(error) => return err_json(format!("economy store: {error}")),
    };
    let holdings = dispatch(object(vec![("cmd", str_value("items"))])).get("data").clone();
    let market = dispatch(object(vec![
        ("cmd", str_value("market")),
        ("op", str_value("stats")),
    ]));
    let accounts: Vec<String> = dispatch(object(vec![("cmd", str_value("accounts"))]))
        .get("accounts")
        .as_arr()
        .map(|rows| rows.iter().filter_map(|row| row.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let mut settled = 0i64;
    let mut failed = 0i64;
    for account in &accounts {
        if let Ok(Some(row)) = store.get_account(account) {
            settled += row.deals_settled;
            failed += row.deals_failed;
        }
    }
    let deals_total = settled + failed;
    let mut trends = Vec::new();
    if let Ok(catalogue) = store.list_items() {
        for item in &catalogue {
            for trend in store.trends_for_item(item.item_id).unwrap_or_default() {
                if trend.period == "24h" && trend.score != 0.0 {
                    trends.push((item.item_id, item.name.clone(), trend.shortage, trend.momentum, trend.score));
                }
            }
        }
    }
    trends.sort_by(|left, right| right.4.abs().total_cmp(&left.4.abs()));
    trends.truncate(15);
    let rates = crate::rates_service::rates_api(&**dispatch);
    let mut activities: Vec<(String, i64, f64)> = Vec::new();
    let mut flip: Option<(String, f64, bool)> = None;
    for row in rates.get("profit").as_arr().cloned().unwrap_or_default() {
        let method = row.get("method").as_str().unwrap_or("").to_string();
        let profit = row.get("profit").as_f64().unwrap_or(0.0);
        if method == "market:flip" || (flip.is_none() && method == "market") {
            flip = Some((method.clone(), profit, row.get("price_prior").as_bool().unwrap_or(true)));
        }
        activities.push((method, row.get("band").as_i64().unwrap_or(0), profit));
    }
    activities.sort_by(|left, right| right.2.total_cmp(&left.2));
    activities.truncate(10);
    let mut merchants = Vec::new();
    if let Ok(entries) = std::fs::read_dir(lives_dir(run_dir)) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        for name in names {
            let account = name[..name.len() - 5].to_string();
            let text = std::fs::read_to_string(lives_dir(run_dir).join(&name)).unwrap_or_default();
            let life = json::parse(&text).unwrap_or(Value::Null);
            if life.get("profile").get("archetype").as_str() != Some("merchant") {
                continue;
            }
            let (buy_value, sell_qty, sell_value, open_orders, items) = merchant_exposure(dispatch, &account);
            let open_deals = dispatch(object(vec![
                ("cmd", str_value("market")),
                ("op", str_value("my_deals")),
                ("account", str_value(&account)),
            ]))
            .get("deals")
            .as_arr()
            .map(|rows| {
                rows.iter()
                    .filter(|row| !matches!(row.get("status").as_str(), Some("settled") | Some("failed")))
                    .count()
            })
            .unwrap_or(0);
            let (deal_settled, deal_failed) = store
                .get_account(&account)
                .ok()
                .flatten()
                .map(|row| (row.deals_settled, row.deals_failed))
                .unwrap_or((0, 0));
            merchants.push(object(vec![
                ("account", str_value(&account)),
                ("buy_value", Value::Int(buy_value)),
                ("sell_qty", Value::Int(sell_qty)),
                ("sell_value", Value::Int(sell_value)),
                ("settled", Value::Int(deal_settled)),
                ("failed", Value::Int(deal_failed)),
                ("open_orders", Value::Int(open_orders as i64)),
                ("open_deals", Value::Int(open_deals as i64)),
                ("book_share", Value::Float(merchant_book_share(dispatch, &account, &items))),
            ]));
        }
    }
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    object(vec![
        ("ok", Value::Bool(true)),
        ("clock", Value::Int(clock)),
        ("holdings", holdings),
        (
            "market",
            object(vec![
                ("items", market.get("items").clone()),
                ("books", market.get("books").clone()),
                ("orders", market.get("orders").clone()),
                ("placed", market.get("placed").clone()),
                ("matched", market.get("matched").clone()),
                ("deals", market.get("deals").clone()),
            ]),
        ),
        (
            "deals",
            object(vec![
                ("total", Value::Int(deals_total)),
                ("settled", Value::Int(settled)),
                ("failed", Value::Int(failed)),
                (
                    "fail_rate",
                    Value::Float(if deals_total > 0 { failed as f64 / deals_total as f64 } else { 0.0 }),
                ),
            ]),
        ),
        (
            "activities",
            Value::Arr(
                activities
                    .iter()
                    .map(|(method, band, profit)| {
                        object(vec![
                            ("method", str_value(method)),
                            ("band", Value::Int(*band)),
                            ("profit", Value::Float(*profit)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "flip",
            flip.map(|(method, profit, prior)| {
                object(vec![
                    ("method", str_value(&method)),
                    ("profit", Value::Float(profit)),
                    ("price_prior", Value::Bool(prior)),
                ])
            })
            .unwrap_or(Value::Null),
        ),
        (
            "trends",
            Value::Arr(
                trends
                    .iter()
                    .map(|(item, name, shortage, momentum, score)| {
                        object(vec![
                            ("item", Value::Int(*item)),
                            ("name", str_value(name)),
                            ("shortage", Value::Float(*shortage)),
                            ("momentum", Value::Float(*momentum)),
                            ("score", Value::Float(*score)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("merchants", Value::Arr(merchants)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn life_and_economy_shapes() {
        let root = std::env::temp_dir().join(format!("lifescape-l10-{}", std::process::id()));
        let run = root.join("run");
        std::fs::create_dir_all(run.join("lives")).expect("scratch lives dir");
        std::fs::write(
            run.join("lives").join("m1.json"),
            r#"{"schema":1,"account":"m1","created_at":1,"seed":9,
                "profile":{"ironman":false,"market_affinity":0.5,"price_patience":0.5,"archetype":"merchant"},
                "goal":{"kind":"get_rich","merchant":true},
                "nodes":[{"id":"n1","kind":"make_money","params":{},"deps":[],"status":"active",
                "method":"market:flip","alternatives":[{"method":"market:flip","est_ticks":6000,"est_coins":0}],
                "progress":{"current":0,"target":0},"spent_ticks":100}],
                "engagement":{"node":"n1","min_ticks":6000,"max_ticks":12000,"chosen":9000},
                "active":"n1","history":[]}"#,
        )
        .expect("scratch life file");
        // Life file verbatim: goal, statuses, method, alternatives, engagement.
        let life = life_response_in(&run, "m1");
        assert_eq!(life.get("ok").as_bool(), Some(true));
        assert_eq!(life.get("life").get("account").as_str(), Some("m1"));
        assert_eq!(life.get("life").get("goal").get("kind").as_str(), Some("get_rich"));
        let nodes = life.get("life").get("nodes").as_arr().expect("nodes array");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].get("status").as_str(), Some("active"));
        assert_eq!(nodes[0].get("method").as_str(), Some("market:flip"));
        assert_eq!(nodes[0].get("alternatives").as_arr().map(|rows| rows.len()), Some(1));
        assert_eq!(life.get("life").get("engagement").get("node").as_str(), Some("n1"));
        // Unknown account and traversal read loud, never the filesystem.
        assert_eq!(life_response_in(&run, "nobody").get("ok").as_bool(), Some(false));
        assert_eq!(life_response_in(&run, "..").get("ok").as_bool(), Some(false));

        // Index: goal plus counts by status.
        let lives = lives_response_in(&run);
        let rows = lives.get("lives").as_arr().expect("lives array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("account").as_str(), Some("m1"));
        assert_eq!(rows[0].get("nodes").get("active").as_i64(), Some(1));

        // Economy: one stub dispatch for every daemon command used.
        fn stub(request: Value) -> Value {
            match request.get("cmd").as_str().unwrap_or("") {
                "status" => object(vec![("jobs", Value::Arr(Vec::new()))]),
                "items" => object(vec![(
                    "data",
                    object(vec![("liveTotal", Value::Int(42)), ("items", Value::Arr(Vec::new()))]),
                )]),
                "accounts" => object(vec![(
                    "accounts",
                    Value::Arr(vec![str_value("m1")]),
                )]),
                _ => match request.get("op").as_str().unwrap_or("") {
                    "stats" => object(vec![
                        ("ok", Value::Bool(true)),
                        ("items", Value::Int(3)),
                        ("books", Value::Int(1)),
                        ("orders", Value::Int(2)),
                        ("placed", Value::Int(2)),
                        ("matched", Value::Int(0)),
                        ("deals", Value::Int(0)),
                    ]),
                    "my_orders" => object(vec![(
                        "orders",
                        Value::Arr(vec![
                            object(vec![
                                ("side", str_value("buy")),
                                ("item", Value::Int(5)),
                                ("left", Value::Int(10)),
                                ("price", Value::Int(4)),
                                ("status", str_value("open")),
                            ]),
                            object(vec![
                                ("side", str_value("sell")),
                                ("item", Value::Int(5)),
                                ("left", Value::Int(6)),
                                ("price", Value::Int(7)),
                                ("status", str_value("open")),
                            ]),
                        ]),
                    )]),
                    "my_deals" => object(vec![("deals", Value::Arr(Vec::new()))]),
                    "book" => object(vec![
                        ("buys", Value::Arr(vec![Value::Arr(vec![Value::Int(4), Value::Int(20)])])),
                        ("sells", Value::Arr(vec![Value::Arr(vec![Value::Int(7), Value::Int(12)])])),
                    ]),
                    _ => object(vec![("ok", Value::Bool(false))]),
                },
            }
        }
        let dispatch: Dispatch = Arc::new(stub);
        let economy = economy_response_in(&dispatch, &run);
        assert_eq!(economy.get("ok").as_bool(), Some(true), "empty store still serves a shape");
        assert_eq!(economy.get("holdings").get("liveTotal").as_i64(), Some(42));
        assert_eq!(economy.get("deals").get("total").as_i64(), Some(0));
        assert_eq!(economy.get("market").get("items").as_i64(), Some(3));
        assert!(economy.get("activities").as_arr().is_some());
        assert!(economy.get("trends").as_arr().is_some());
        let merchants = economy.get("merchants").as_arr().expect("merchants array");
        assert_eq!(merchants.len(), 1, "the merchant life file drives the card");
        assert_eq!(merchants[0].get("account").as_str(), Some("m1"));
        assert_eq!(merchants[0].get("buy_value").as_i64(), Some(40));
        assert_eq!(merchants[0].get("sell_qty").as_i64(), Some(6));
        assert_eq!(merchants[0].get("sell_value").as_i64(), Some(42));
        assert_eq!(merchants[0].get("open_orders").as_i64(), Some(2));
        // 10 of 20 resting bids, 6 of 12 asks: share 0.5.
        assert!((merchants[0].get("book_share").as_f64().unwrap_or(-1.0) - 0.5).abs() < 1e-9);

        let _ = std::fs::remove_dir_all(&root);
    }
}
