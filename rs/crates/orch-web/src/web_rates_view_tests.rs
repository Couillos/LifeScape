//! Rates page API shapes: query parsing, page JSON (headline lines, not
//! every flow), detail JSON, index freshness.

use super::*;
use crate::rates_estimator::{RunSample, YieldBook, YieldKey};

fn index() -> RatesIndex {
    let mut book = YieldBook::default();
    let key = YieldKey { method: "fight:melee/cows".to_string(), style: "melee:strength".to_string(), site: "3255,3270,0".to_string(), tool: "iron_sword".to_string(), band: 11 };
    let flows: Vec<(String, String, i64)> = [("Cowhide", "loot", 50), ("Bones", "loot", 50), ("Raw beef", "loot", 50), ("Coins", "loot", 30), ("Trout", "food", -5)].iter().map(|(item, cause, qty)| (item.to_string(), cause.to_string(), *qty)).collect();
    for _ in 0..3 {
        book.record(&key, &RunSample { work_ticks: 6000, pause_ticks: 0, flows: flows.clone(), units: vec![("damage".to_string(), "cow".to_string(), 400)], clock: 10 });
    }
    build_index(&book, 70, &mut |item: &str| if item == "Cowhide" { 100.0 } else { 5.0 })
}

fn params(query: &str) -> HashMap<String, String> {
    crate::web::parse_query(query)
}

#[test]
fn query_parameters_are_decoded() {
    let query = view_query_of(&params("group=key&q=raw%20beef&skill=strength&family=fight&band=11&measured=1&min_runs=2&sort=xp_total&dir=asc&offset=50&limit=25"));
    assert_eq!(query.group, Group::Key);
    assert_eq!(query.text, "raw beef");
    assert_eq!((query.skill.as_deref(), query.family.as_deref(), query.band), (Some("strength"), Some("fight"), Some(11)));
    assert!(query.measured_only);
    assert_eq!((query.min_runs, query.sort.as_str(), query.descending, query.offset, query.limit), (2, "xp_total", false, 50, 25));
    let defaults = view_query_of(&params(""));
    assert_eq!(defaults, ViewQuery { limit: 0, ..ViewQuery::default() }, "no parameter: one row per activity, most runs first");
}

/// A list row carries the headline lines only (3 products, 3
/// consumption) and the count of its flows; the detail every flow.
#[test]
fn page_rows_carry_headlines_and_detail_every_flow() {
    let index = index();
    let page = object(activities_value(&index, &ViewQuery::default()));
    assert_eq!(page.get("matched").as_i64(), Some(1));
    assert_eq!(page.get("total").as_i64(), Some(1));
    let row = &page.get("rows").as_arr().cloned().unwrap_or_default()[0];
    assert_eq!(row.get("products").as_arr().map(|lines| lines.len()), Some(HEADLINE_LINES));
    assert_eq!(row.get("products").as_arr().and_then(|lines| lines[0].get("item").as_str().map(str::to_string)).as_deref(), Some("Cowhide"), "best value first");
    assert_eq!(row.get("consumption").as_arr().map(|lines| lines.len()), Some(1));
    assert_eq!(row.get("flow_count").as_i64(), Some(5));
    assert_eq!(row.get("age_secs").as_i64(), Some(60));
    assert!(row.get("xp").get("strength").as_f64().unwrap_or(0.0) > 0.0, "derived XP of the style");
    assert!(row.get("flows").as_arr().is_none(), "no full flow list in a page");
    let detail = object(detail_value(&index, &params("method=fight%3Amelee%2Fcows&style=melee%3Astrength")));
    assert_eq!(detail.get("ok").as_bool(), Some(true), "{}", lifescape_json::stringify(&detail));
    assert_eq!(detail.get("summary").get("flows").as_arr().map(|lines| lines.len()), Some(5));
    assert_eq!(detail.get("keys").as_arr().map(|keys| keys.len()), Some(1));
    assert_eq!(detail.get("keys").as_arr().and_then(|keys| keys[0].get("site").as_str().map(str::to_string)).as_deref(), Some("3255,3270,0"));
    let missing = object(detail_value(&index, &params("method=nope")));
    assert_eq!(missing.get("ok").as_bool(), Some(false));
}

/// The index serves under `INDEX_TTL` even when reports land, up to
/// `PRICE_TTL` when none does.
#[test]
fn index_freshness() {
    let built = Instant::now();
    let cached = CachedIndex { built, generation: 4, build_ms: 1.0, index: Arc::new(RatesIndex::default()) };
    assert!(cached.fresh(built + INDEX_TTL - Duration::from_millis(1), 5), "new reports wait for the TTL");
    assert!(!cached.fresh(built + INDEX_TTL, 5), "new reports after the TTL: rebuild");
    assert!(cached.fresh(built + INDEX_TTL, 4), "no report: kept");
    assert!(!cached.fresh(built + PRICE_TTL, 4), "prices move: rebuild");
}
