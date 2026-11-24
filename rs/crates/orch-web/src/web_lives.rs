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

/// Every account with a life file: one flat row per account (PRD 2,
/// §6.4, D47). Column order follows `LIVES_COLUMNS`; counts cover the
/// six frozen statuses (an `abandoned` node is a replaced method, not a
/// finished one, so it counts nowhere).
pub fn lives_response() -> Value {
    lives_response_in(&crate::paths::run_dir())
}

fn life_updated(life: &Value, file_modified: i64) -> i64 {
    let mut updated = file_modified;
    for entry in life.get("history").as_arr().cloned().unwrap_or_default() {
        if let Some(ended) = entry.get("ended").as_i64() {
            updated = updated.max(ended);
        }
    }
    updated
}

/// Active production chain, oldest link first. The brain (P2.5) will
/// write an explicit `chain` array; until then the dependency path of
/// the active node stands in (each link's method, active last).
fn life_chain(life: &Value) -> Vec<String> {
    if let Some(explicit) = life.get("chain").as_arr() {
        let links: Vec<String> =
            explicit.iter().filter_map(|link| link.as_str().map(str::to_string)).collect();
        if !links.is_empty() {
            return links;
        }
    }
    let nodes = life.get("nodes").as_arr().cloned().unwrap_or_default();
    let by_id: std::collections::HashMap<String, &Value> = nodes
        .iter()
        .filter_map(|node| node.get("id").as_str().map(|id| (id.to_string(), node)))
        .collect();
    let Some(mut current) = life.get("active").as_str().map(str::to_string) else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    for _ in 0..nodes.len().saturating_add(1) {
        let Some(node) = by_id.get(&current) else { break };
        if let Some(method) = node.get("method").as_str() {
            if !method.is_empty() && chain.last().is_none_or(|last: &String| last != method) {
                chain.push(method.to_string());
            }
        }
        let Some(next) = node.get("deps").as_arr().and_then(|deps| deps.first()).and_then(|dep| dep.as_str()) else {
            break;
        };
        if next == current || !by_id.contains_key(next) {
            break;
        }
        current = next.to_string();
    }
    chain.reverse();
    chain
}

fn life_row(account: &str, life: &Value, file_modified: i64) -> Value {
    let mut counts = std::collections::BTreeMap::new();
    for node in life.get("nodes").as_arr().cloned().unwrap_or_default() {
        let status = node.get("status").as_str().unwrap_or("pending");
        if crate::api_shapes::LIVES_COLUMNS.iter().skip(6).any(|column| *column == status) {
            *counts.entry(status.to_string()).or_insert(0i64) += 1;
        }
    }
    let row = crate::api_shapes::LivesRow {
        account: account.to_string(),
        goal: life.get("goal").clone(),
        profile: life.get("profile").clone(),
        active_task: life.get("active").as_str().map(str::to_string),
        chain: life_chain(life),
        engagement: if life.get("engagement").is_null() { None } else { Some(life.get("engagement").clone()) },
        active: counts.get("active").copied().unwrap_or(0),
        ready: counts.get("ready").copied().unwrap_or(0),
        pending: counts.get("pending").copied().unwrap_or(0),
        blocked: counts.get("blocked").copied().unwrap_or(0),
        failed: counts.get("failed").copied().unwrap_or(0),
        // Retired done nodes (`memory.done_total`, 2026-11-14) count too.
        done: counts.get("done").copied().unwrap_or(0) + life.get("memory").get("done_total").as_i64().unwrap_or(0),
        updated: life_updated(life, file_modified),
    };
    row.to_value()
}

/// Life index (see `lives_response`) over an explicit root.
pub fn lives_response_in(run_dir: &std::path::Path) -> Value {
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(lives_dir(run_dir)) {
        let mut names: Vec<std::fs::DirEntry> =
            entries.flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".json")).collect();
        names.sort_by_key(|entry| entry.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().to_string();
            let account = name[..name.len() - 5].to_string();
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(life) = json::parse(&text) else {
                continue;
            };
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|span| span.as_secs() as i64)
                .unwrap_or(0);
            rows.push(life_row(&account, &life, modified));
        }
    }
    object(vec![("ok", Value::Bool(true)), ("lives", Value::Arr(rows))])
}

/// Lines of the economy page's « top activities ».
pub const TOP_ACTIVITIES: usize = 10;

/// « Top activities » of the economy page: the money guide's catalogue
/// read from the precomputed set (`guides_store`, the guides page's own
/// figures, never the old rates profit table): its priced lines with no
/// condition impossible for every bot (D22), best profit/6000 first,
/// `TOP_ACTIVITIES` at most, with family, method, band and confidence.
/// A general top: priced lines with nothing impossible for every bot and
/// no stock to hold first (a recipe on stock is judged on a bot's own
/// stock, D22; its D5 value of a never-traded input would top the list).
pub fn guide_activities(set: &crate::guides_store::GuideSet) -> Vec<Value> {
    let Ok((rows, _)) = set.money(&lifescape_planner::guide_model::GuideFilter::default()) else { return Vec::new() };
    let mut kept: Vec<&lifescape_planner::guide_model::MoneyRow> = rows
        .iter()
        .filter(|row| row.priced && row.profit_6000.is_finite() && row.prereqs.stock.is_empty() && !lifescape_planner::guide_feasible::conditions(&row.prereqs).iter().any(|condition| condition.impossible()))
        .collect();
    kept.sort_by(|left, right| right.profit_6000.total_cmp(&left.profit_6000));
    kept.truncate(TOP_ACTIVITIES);
    kept.iter()
        .map(|row| {
            object(vec![
                ("family", str_value(row.family.as_str())),
                ("method", str_value(&row.method)),
                ("style", str_value(&row.style)),
                ("site", str_value(&crate::guides_model::site_text(row.site))),
                ("band", Value::Int(row.band)),
                ("profit_6000", Value::Float(row.profit_6000)),
                ("confidence", crate::guides_model::confidence_value(&row.confidence)),
            ])
        })
        .collect()
}

/// Economy payload from existing sources only: market stats via the
/// `market` command, deals and market makers via the store and the life
/// files (money supply, top holdings and trends left the page,
/// 2026-11-17: the brain reads the store's trends itself), top activities via the
/// precomputed money guide (`guide_activities`). One JSON fetch for
/// the economy page; bots never read it.
pub fn economy_response(dispatch: &Dispatch) -> Value {
    economy_response_in(dispatch, &crate::paths::run_dir())
}

/// Economy payload (see `economy_response`) over an explicit root.
pub fn economy_response_in(dispatch: &Dispatch, run_dir: &std::path::Path) -> Value {
    let store = match crate::store_pool::open(&run_dir.join("lifescape.db")) {
        Ok(store) => store,
        Err(error) => return err_json(format!("economy store: {error}")),
    };
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
    // Top activities: the money guide's set; none before its first build.
    let guides = crate::guides_store::current();
    let activities = guides.as_deref().map(guide_activities).unwrap_or_default();
    // Market makers (plan marché Q6): operator info, never read by a bot.
    let makers: Vec<Value> = crate::autopilot::current().maker_rows(&store, &lives_dir(run_dir));
    // 100 % market lots (plan rendements R5): predicted vs realized.
    let trade_rows = crate::trade_report::trade_rows(&store, &lives_dir(run_dir));
    let (closed, predicted, realized) = crate::trade_report::closed_totals(&trade_rows);
    let trades: Vec<Value> = trade_rows.iter().take(50).map(crate::trade_report::row_value).collect();
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs() as i64).unwrap_or(0);
    // Cancelled / failed transactions and their percentage (staff and
    // personal accounts excluded, measure rule).
    let excluded: Vec<String> = crate::economy_guard::excluded_accounts().into_iter().collect();
    let bounds = crate::economy_outcomes::since_bounds(clock);
    let outcomes = crate::economy_outcomes::cached_outcomes(&run_dir.join("lifescape.db"), clock, || {
        let deal_rows = store.deal_outcomes(&bounds, &excluded)?;
        let order_rows = store.order_outcomes(&bounds, &excluded)?;
        Ok(crate::economy_outcomes::outcomes_value(&deal_rows, &order_rows))
    });
    let outcomes = match outcomes {
        Ok(value) => value,
        Err(error) => return err_json(format!("economy outcomes: {error}")),
    };
    object(vec![
        ("ok", Value::Bool(true)),
        ("clock", Value::Int(clock)),
        ("outcomes", outcomes),
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
        ("activities", Value::Arr(activities)),
        ("activities_pending", Value::Bool(guides.is_none())),
        ("makers", Value::Arr(makers)),
        ("trades", Value::Arr(trades)),
        ("trades_closed", object(vec![("lots", Value::Int(closed as i64)), ("predicted", Value::Int(predicted)), ("realized", Value::Int(realized))])),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn life_updated_takes_the_max_of_history_and_mtime() {
        let empty = object(vec![]);
        assert_eq!(life_updated(&empty, 100), 100);
        let life = object(vec![(
            "history",
            Value::Arr(vec![
                object(vec![("ended", Value::Int(50))]),
                object(vec![("ended", Value::Int(300))]),
                object(vec![("outcome", str_value("goal_reached"))]),
            ]),
        )]);
        assert_eq!(life_updated(&life, 100), 300, "a newer history entry beats the file mtime");
        assert_eq!(life_updated(&life, 500), 500, "a newer mtime beats history");
    }

    #[test]
    fn lives_row_derives_chain_counts_and_updated() {
        let root = std::env::temp_dir().join(format!("lifescape-l11-{}", std::process::id()));
        let run = root.join("run");
        std::fs::create_dir_all(run.join("lives")).expect("scratch lives dir");
        std::fs::write(
            run.join("lives").join("t01.json"),
            r#"{"account":"t01","goal":{"kind":"make_money"},"profile":{"ironman":false},
                "nodes":[
                 {"id":"n1","status":"done","method":"mining/iron","deps":[]},
                 {"id":"n2","status":"active","method":"smelting/iron","deps":["n1"]},
                 {"id":"n3","status":"abandoned","method":"smithing/plate","deps":["n2"]}],
                "engagement":{"node":"n2","min_ticks":6000,"max_ticks":12000,"chosen":9000},
                "active":"n2","history":[{"node":"n1","ended":1700000000,"outcome":"goal_reached"}]}"#,
        )
        .expect("scratch life file");
        std::fs::write(run.join("lives").join("t02.json"), r#"{"account":"t02","goal":null,"nodes":[],"active":null}"#)
            .expect("scratch life file");
        let lives = lives_response_in(&run);
        let rows = lives.get("lives").as_arr().expect("lives array");
        assert_eq!(rows.len(), 2);
        let first = &rows[0];
        assert_eq!(first.get("account").as_str(), Some("t01"));
        let chain: Vec<String> =
            first.get("chain").as_arr().unwrap().iter().filter_map(|link| link.as_str().map(str::to_string)).collect();
        assert_eq!(chain, vec!["mining/iron".to_string(), "smelting/iron".to_string()]);
        assert_eq!(first.get("done").as_i64(), Some(1), "abandoned counts nowhere");
        assert_eq!(first.get("engagement").get("node").as_str(), Some("n2"));
        assert!(first.get("updated").as_i64().unwrap_or(0) >= 1700000000, "last change covers history and mtime");
        let second = &rows[1];
        assert_eq!(second.get("active_task"), &Value::Null);
        assert!(second.get("chain").as_arr().map(|links| links.is_empty()).unwrap_or(false));
        assert_eq!(second.get("goal"), &Value::Null);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The economy page's maker card is the brain's rows (`Autopilot::maker_rows`,
    /// whose real source is proved in `orch-brain`, `market_maker_report`):
    /// here a double stands in and the page must pass its rows through.
    struct MakerDouble;

    impl lifescape_orch_fleet::autopilot::Autopilot for MakerDouble {
        fn maker_rows(&self, _store: &lifescape_store::Store, _lives_dir: &std::path::Path) -> Vec<Value> {
            vec![object(vec![("account", str_value("m1")), ("phase", str_value("quoting")), ("slice", Value::Int(4)), ("capital", Value::Int(91_000))])]
        }
    }

    #[test]
    fn life_and_economy_shapes() {
        lifescape_orch_fleet::autopilot::register(Arc::new(MakerDouble));
        let root = std::env::temp_dir().join(format!("lifescape-l10-{}", std::process::id()));
        let run = root.join("run");
        std::fs::create_dir_all(run.join("lives")).expect("scratch lives dir");
        std::fs::write(
            run.join("lives").join("m1.json"),
            r#"{"schema":1,"account":"m1","created_at":1,"seed":9,
                "profile":{"ironman":false,"market_affinity":0.5,"price_patience":0.5,"archetype":"market_maker",
                  "maker":{"slice":4,"slices":11,"capital_target":90000,"phase":"quoting","capital_at_switch":91000,"switched_at":1}},
                "goal":{"kind":"make_market"},
                "nodes":[{"id":"n1","kind":"make_money","params":{},"deps":[],"status":"active",
                "method":"market:make","alternatives":[{"method":"market:make","est_ticks":6000,"est_coins":0}],
                "progress":{"current":0,"target":0},"spent_ticks":100}],
                "engagement":{"node":"n1","min_ticks":6000,"max_ticks":12000,"chosen":9000},
                "active":"n1","history":[]}"#,
        )
        .expect("scratch life file");
        // Life file verbatim: goal, statuses, method, alternatives, engagement.
        let life = life_response_in(&run, "m1");
        assert_eq!(life.get("ok").as_bool(), Some(true));
        assert_eq!(life.get("life").get("account").as_str(), Some("m1"));
        assert_eq!(life.get("life").get("goal").get("kind").as_str(), Some("make_market"));
        let nodes = life.get("life").get("nodes").as_arr().expect("nodes array");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].get("status").as_str(), Some("active"));
        assert_eq!(nodes[0].get("method").as_str(), Some("market:make"));
        assert_eq!(nodes[0].get("alternatives").as_arr().map(|rows| rows.len()), Some(1));
        assert_eq!(life.get("life").get("engagement").get("node").as_str(), Some("n1"));
        // Unknown account and traversal read loud, never the filesystem.
        assert_eq!(life_response_in(&run, "nobody").get("ok").as_bool(), Some(false));
        assert_eq!(life_response_in(&run, "..").get("ok").as_bool(), Some(false));

        // Index: one flat row per account (§6.4, D47), every frozen
        // column present, sortable counts plus last change.
        let lives = lives_response_in(&run);
        let rows = lives.get("lives").as_arr().expect("lives array");
        assert_eq!(rows.len(), 1);
        let mut got: Vec<String> = rows[0].as_obj().map(|map| map.keys().cloned().collect()).unwrap_or_default();
        got.sort();
        let mut want: Vec<String> = crate::api_shapes::LIVES_COLUMNS.iter().map(|key| key.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "lives row covers every §6.4 column");
        assert_eq!(rows[0].get("account").as_str(), Some("m1"));
        assert_eq!(rows[0].get("goal").get("kind").as_str(), Some("make_market"));
        assert_eq!(rows[0].get("profile").get("archetype").as_str(), Some("market_maker"));
        assert_eq!(rows[0].get("active_task").as_str(), Some("n1"));
        assert_eq!(rows[0].get("active").as_i64(), Some(1));
        assert_eq!(rows[0].get("ready").as_i64(), Some(0));
        assert_eq!(rows[0].get("done").as_i64(), Some(0));
        assert!(rows[0].get("updated").as_i64().unwrap_or(0) > 0, "mtime feeds the last change");
        let chain: Vec<String> =
            rows[0].get("chain").as_arr().unwrap().iter().filter_map(|link| link.as_str().map(str::to_string)).collect();
        assert_eq!(chain, vec!["market:make".to_string()], "active method is the chain until the brain writes one");

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
        assert!(economy.get("holdings").is_null(), "money supply and top holdings left the page");
        assert_eq!(economy.get("deals").get("total").as_i64(), Some(0));
        assert_eq!(economy.get("outcomes").get("deals").as_arr().map(|windows| windows.len()), Some(4), "four periods of exchanges");
        assert_eq!(economy.get("outcomes").get("orders").as_arr().map(|windows| windows.len()), Some(4), "four periods of offers");
        assert_eq!(economy.get("market").get("items").as_i64(), Some(3));
        assert!(economy.get("activities").as_arr().is_some());
        assert!(economy.get("activities_pending").as_bool().is_some(), "top activities say when the guides set is not built yet");
        assert!(economy.get("trends").is_null(), "the trends card left the page (the brain reads the store's trends)");
        let makers = economy.get("makers").as_arr().expect("makers array");
        assert_eq!(makers.len(), 1, "the maker life file drives the card");
        assert_eq!(makers[0].get("account").as_str(), Some("m1"));
        assert_eq!(makers[0].get("phase").as_str(), Some("quoting"));
        assert_eq!(makers[0].get("slice").as_i64(), Some(4));
        assert_eq!(makers[0].get("capital").as_i64(), Some(91_000), "no deal yet: the capital at its switch");
        assert!(economy.get("flip").is_null() && economy.get("merchants").is_null(), "the merchant card is gone");

        let _ = std::fs::remove_dir_all(&root);
    }
}
