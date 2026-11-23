//! Rates page view (user 2026-11-17): one row per activity (or per
//! exact key), filters, server pagination and sort, the detail of one
//! activity with its keys as sub-levels.

use super::*;
use crate::rates_estimator::{RunSample, YieldBook, YieldKey};

fn key(method: &str, style: &str, site: &str, tool: &str, band: i64) -> YieldKey {
    YieldKey { method: method.to_string(), style: style.to_string(), site: site.to_string(), tool: tool.to_string(), band }
}

fn run(ticks: i64, flows: &[(&str, &str, i64)], units: &[(&str, &str, i64)]) -> RunSample {
    RunSample {
        work_ticks: ticks,
        pause_ticks: 0,
        flows: flows.iter().map(|(item, cause, qty)| (item.to_string(), cause.to_string(), *qty)).collect(),
        units: units.iter().map(|(kind, subject, qty)| (kind.to_string(), subject.to_string(), *qty)).collect(),
        clock: 1_000,
    }
}

fn chickens(book: &mut YieldBook, site: &str, band: i64, runs: usize, feathers: i64) {
    for _ in 0..runs {
        book.record(&key("fight:melee/chickens", "melee:attack", site, "bronze_sword", band), &run(6000, &[("Feather", "loot", feathers), ("Bones", "loot", 40), ("Shrimps", "food", -2)], &[("kill", "chicken", 40)]));
    }
}

fn logs(book: &mut YieldBook, runs: usize) {
    for _ in 0..runs {
        book.record(&key("gather:woodcut/logs", "", "3100,3230,0", "bronze_axe", 1), &run(6000, &[("Logs", "product", 300)], &[("product", "Logs", 300)]));
    }
}

fn price(item: &str) -> f64 {
    match item {
        "Feather" => 3.0,
        "Bones" => 50.0,
        "Shrimps" => 20.0,
        "Logs" => 10.0,
        _ => 1.0,
    }
}

/// Chickens at two sites and two bands (4 exact keys, the first with 3
/// runs), logs with one key of 1 run.
fn book() -> YieldBook {
    let mut book = YieldBook::default();
    chickens(&mut book, "3230,3295,0", 1, 3, 100);
    chickens(&mut book, "3230,3295,0", 6, 1, 100);
    chickens(&mut book, "3177,3360,0", 1, 1, 100);
    chickens(&mut book, "3177,3360,0", 6, 1, 100);
    logs(&mut book, 1);
    book
}

fn index() -> RatesIndex {
    build_index(&book(), 2_000, &mut |item: &str| price(item))
}

fn methods(page: &Page) -> Vec<String> {
    page.rows.iter().map(|row| row.method.clone()).collect()
}

/// The book holds 5 exact keys and 6 roll-ups: the view lists the exact
/// keys only, one row per activity (method × style) by default.
#[test]
fn one_row_per_activity() {
    let index = index();
    assert_eq!(index.entries.len(), 5, "exact keys only, roll-ups stay internal");
    let page = query_page(&index, &ViewQuery::default());
    assert_eq!(page.total, 2);
    assert_eq!(page.matched, 2);
    let chickens = page.rows.iter().find(|row| row.method == "fight:melee/chickens").expect("chickens row");
    assert_eq!(chickens.keys, 4);
    assert_eq!(chickens.measured_keys, 1);
    assert_eq!(chickens.runs, 6);
    assert_eq!(chickens.sites, 2);
    assert_eq!((chickens.band_min, chickens.band_max), (1, 6));
    assert_eq!(chickens.family, "fight");
}

/// `group=key`: one row per exact key, with its site, tool and band.
#[test]
fn key_group_lists_every_exact_key() {
    let page = query_page(&index(), &ViewQuery { group: Group::Key, ..ViewQuery::default() });
    assert_eq!(page.total, 5);
    assert!(page.rows.iter().all(|row| row.site.as_deref().is_some_and(|site| site != "*") && row.tool.as_deref().is_some_and(|tool| tool != "*")));
}

/// An activity's figures are Σ / Σ work ticks over its keys (each key's
/// rate weighted by its work), never a mean of rates.
#[test]
fn activity_figures_are_work_weighted() {
    let mut book = YieldBook::default();
    chickens(&mut book, "a", 1, 2, 100);
    for _ in 0..1 {
        book.record(&key("fight:melee/chickens", "melee:attack", "b", "bronze_sword", 1), &run(3000, &[("Feather", "loot", 200)], &[]));
    }
    let index = build_index(&book, 2_000, &mut |item: &str| price(item));
    let page = query_page(&index, &ViewQuery::default());
    let feathers = page.rows[0].flows.iter().find(|line| line.item == "Feather").expect("feathers");
    // Key a: 100 / 6000 (work 2 × 6000 decayed); key b: 400 / 6000 over 3000 ticks.
    let entry_a = index.entries.iter().find(|entry| entry.key.site == "a").expect("a");
    let entry_b = index.entries.iter().find(|entry| entry.key.site == "b").expect("b");
    let expected = (100.0 * entry_a.work_ticks + 400.0 * entry_b.work_ticks) / (entry_a.work_ticks + entry_b.work_ticks);
    assert!((feathers.per_6000 - expected).abs() < 1e-6, "{} vs {expected}", feathers.per_6000);
    assert!((feathers.value_6000 - expected * 3.0).abs() < 1e-6);
}

/// Headline columns: products (in, best value first), consumption
/// (out), derived XP per skill, net value.
#[test]
fn headline_products_consumption_and_xp() {
    let page = query_page(&index(), &ViewQuery { text: "logs".to_string(), ..ViewQuery::default() });
    let row = &page.rows[0];
    assert_eq!(row.method, "gather:woodcut/logs");
    assert_eq!(row.products().first().map(|line| line.item.as_str()), Some("Logs"));
    assert!(row.consumption().is_empty());
    assert!(row.xp.iter().any(|(skill, rate)| skill == "woodcutting" && (*rate - 300.0 * 25.0).abs() < 1e-6), "{:?}", row.xp);
    assert!((row.net_value_6000 - 3000.0).abs() < 1e-6);
    let page = query_page(&index(), &ViewQuery { text: "chickens".to_string(), ..ViewQuery::default() });
    let row = &page.rows[0];
    assert_eq!(row.products().iter().map(|line| line.item.as_str()).collect::<Vec<_>>(), vec!["Bones", "Feather"], "best value first");
    assert_eq!(row.consumption().iter().map(|line| line.item.as_str()).collect::<Vec<_>>(), vec!["Shrimps"]);
    assert!((row.consumption_value_6000 + 40.0).abs() < 1e-6, "{}", row.consumption_value_6000);
}

/// Filters: text (method, item, site), skill (derived XP), family,
/// band, measured only (n >= 3), minimum runs.
#[test]
fn filters_keep_matching_keys() {
    let index = index();
    let only = |query: ViewQuery| methods(&query_page(&index, &query));
    assert_eq!(only(ViewQuery { text: "feather".to_string(), ..ViewQuery::default() }), vec!["fight:melee/chickens"], "an item name");
    assert_eq!(only(ViewQuery { text: "3100,3230".to_string(), ..ViewQuery::default() }), vec!["gather:woodcut/logs"], "a site");
    assert_eq!(only(ViewQuery { skill: Some("woodcutting".to_string()), ..ViewQuery::default() }), vec!["gather:woodcut/logs"]);
    assert_eq!(only(ViewQuery { family: Some("fight".to_string()), ..ViewQuery::default() }), vec!["fight:melee/chickens"]);
    assert_eq!(only(ViewQuery { measured_only: true, ..ViewQuery::default() }), vec!["fight:melee/chickens"]);
    let page = query_page(&index, &ViewQuery { measured_only: true, ..ViewQuery::default() });
    assert_eq!((page.rows[0].keys, page.rows[0].runs, page.total, page.matched), (1, 3, 2, 1), "the activity folds its measured keys only");
    let page = query_page(&index, &ViewQuery { band: Some(6), ..ViewQuery::default() });
    assert_eq!(methods(&page), vec!["fight:melee/chickens"]);
    assert_eq!((page.rows[0].keys, page.rows[0].band_min), (2, 6));
    assert_eq!(only(ViewQuery { min_runs: 2, ..ViewQuery::default() }), vec!["fight:melee/chickens"]);
    assert!(only(ViewQuery { min_runs: 4, ..ViewQuery::default() }).is_empty());
}

/// Server sort on any column, then the page: offset and limit, the
/// count of matched rows and of every row.
#[test]
fn sort_and_pagination() {
    let index = index();
    let page = query_page(&index, &ViewQuery { group: Group::Key, sort: "runs".to_string(), descending: true, limit: 2, ..ViewQuery::default() });
    assert_eq!((page.total, page.matched, page.rows.len()), (5, 5, 2));
    assert_eq!(page.rows[0].runs, 3, "most runs first");
    let page = query_page(&index, &ViewQuery { group: Group::Key, sort: "runs".to_string(), descending: true, offset: 4, limit: 2, ..ViewQuery::default() });
    assert_eq!(page.rows.len(), 1, "last page");
    let page = query_page(&index, &ViewQuery { sort: "method".to_string(), descending: false, ..ViewQuery::default() });
    assert_eq!(methods(&page), vec!["fight:melee/chickens", "gather:woodcut/logs"]);
    let page = query_page(&index, &ViewQuery { sort: "net_value_6000".to_string(), descending: true, ..ViewQuery::default() });
    assert_eq!(methods(&page), vec!["gather:woodcut/logs", "fight:melee/chickens"]);
    let page = query_page(&index, &ViewQuery { sort: "no_such_column".to_string(), ..ViewQuery::default() });
    assert_eq!(page.sort, DEFAULT_SORT, "an unknown column sorts by the default");
}

/// Facets list what the filters can take (families, skills, bands).
#[test]
fn facets_of_the_index() {
    let facets = facets(&index());
    assert_eq!(facets.families, vec!["fight", "gather"]);
    assert_eq!(facets.bands, vec![1, 6]);
    assert!(facets.skills.contains(&"woodcutting".to_string()), "{:?}", facets.skills);
}

/// Detail of one activity: every flow (rare pickups folded into one
/// valued "other drops" line), the units, the derived XP, and each key
/// as a sub-row (site × tool × band); a key given narrows to it.
#[test]
fn detail_of_an_activity_and_of_a_key() {
    let mut book = book();
    book.record(&key("fight:melee/chickens", "melee:attack", "3230,3295,0", "bronze_sword", 1), &run(6000, &[("Feather", "loot", 100), ("Bones", "loot", 40), ("Bronze arrow", "loot", 1)], &[]));
    let index = build_index(&book, 2_000, &mut |item: &str| price(item));
    let target = DetailTarget { method: "fight:melee/chickens".to_string(), style: "melee:attack".to_string(), ..DetailTarget::default() };
    let detail = query_detail(&index, &target, &ViewQuery::default()).expect("detail");
    assert_eq!(detail.keys.len(), 4, "every key of the activity");
    assert!(detail.summary.flows.iter().any(|line| line.folded > 0 && line.item.starts_with(lifescape_planner::guide_flows::OTHER_DROPS)), "{:?}", detail.summary.flows);
    assert!(!detail.summary.units.is_empty());
    let narrowed = DetailTarget { site: Some("3177,3360,0".to_string()), band: Some(6), ..target.clone() };
    let detail = query_detail(&index, &narrowed, &ViewQuery::default()).expect("key detail");
    assert_eq!((detail.keys.len(), detail.summary.runs), (1, 1));
    let measured = query_detail(&index, &target, &ViewQuery { measured_only: true, ..ViewQuery::default() }).expect("measured keys");
    assert_eq!(measured.keys.len(), 1, "the page's filters apply to the sub-rows");
    assert!(query_detail(&index, &DetailTarget { method: "nope".to_string(), ..DetailTarget::default() }, &ViewQuery::default()).is_none());
}

/// Failure rate of an activity: Σ failed / Σ finished over its keys;
/// age: its freshest key.
#[test]
fn failure_rate_and_age() {
    let mut book = book();
    book.record_failure(&key("gather:woodcut/logs", "", "3100,3230,0", "bronze_axe", 1), 600);
    let index = build_index(&book, 2_000, &mut |item: &str| price(item));
    let page = query_page(&index, &ViewQuery { text: "woodcut".to_string(), ..ViewQuery::default() });
    assert!((page.rows[0].failure_rate - 0.5).abs() < 0.01, "{}", page.rows[0].failure_rate);
    assert_eq!(page.rows[0].age_secs, 1_000);
}
