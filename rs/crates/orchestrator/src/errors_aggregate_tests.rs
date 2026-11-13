use super::*;

fn row(error_id: i64, clock: i64, source: &str, kind: &str, account: &str, signature: &str, zone: &str) -> ErrorRecord {
    ErrorRecord {
        error_id,
        clock,
        source: source.to_string(),
        kind: kind.to_string(),
        account: account.to_string(),
        signature: signature.to_string(),
        zone: zone.to_string(),
        script: if source == "task" { "fishing".to_string() } else { String::new() },
        ..ErrorRecord::default()
    }
}

fn sample() -> Vec<ErrorRecord> {
    vec![
        row(1, 3_600, "nav", "NoRoute", "t01", "no route (#,#,#)", "Varrock"),
        row(2, 3_700, "nav", "NoRoute", "t02", "no route (#,#,#)", "Varrock"),
        row(3, 7_300, "nav", "NoRoute", "t01", "no route (#,#,#)", "Lumbridge"),
        row(4, 7_400, "task", "Failed", "t01", "failed: fish failed at (#,#,#)", "Lumbridge"),
        ErrorRecord { peer: "t03".to_string(), ..row(5, 7_500, "deal", "timeout", "t02", "deal timeout: Iron ore", "Varrock") },
    ]
}

#[test]
fn rates_and_trend() {
    assert_eq!(rate(1, 4), Some(0.25));
    assert_eq!(rate(1, 0), None);
    assert_eq!(trend_pct(15, 10), Some(50.0));
    assert_eq!(trend_pct(5, 10), Some(-50.0));
    assert_eq!(trend_pct(3, 0), None, "no previous window, no trend");
    assert_eq!(trend_pct(2, 3), Some(-33.3));
}

#[test]
fn hourly_buckets_are_zero_filled() {
    let buckets = hourly(&sample(), 3_600, 3 * 3_600);
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].get("t").as_i64(), Some(3_600));
    assert_eq!(buckets[0].get("total").as_i64(), Some(2));
    assert_eq!(buckets[0].get("nav").as_i64(), Some(2));
    assert_eq!(buckets[1].get("total").as_i64(), Some(3));
    assert_eq!(buckets[1].get("deal").as_i64(), Some(1));
    assert_eq!(buckets[1].get("brain").as_i64(), Some(0));
    // Window start mid-hour still starts on the hour.
    assert_eq!(hourly(&[], 5_000, 7_300)[0].get("t").as_i64(), Some(3_600));
}

#[test]
fn groups_count_accounts_zones_and_recency() {
    let rows = sample();
    let grouped = groups(&rows, rows.len() as i64);
    assert_eq!(grouped.len(), 3);
    let first = &grouped[0];
    assert_eq!(first.get("signature").as_str(), Some("no route (#,#,#)"));
    assert_eq!(first.get("count").as_i64(), Some(3));
    assert_eq!(first.get("accounts").as_i64(), Some(2));
    assert_eq!(first.get("top_zone").as_str(), Some("Varrock"));
    assert_eq!(first.get("first_seen").as_i64(), Some(3_600));
    assert_eq!(first.get("last_seen").as_i64(), Some(7_300));
    assert_eq!(first.get("last_id").as_i64(), Some(3));
    assert_eq!(first.get("share").as_f64(), Some(0.6));
    assert_eq!(first.get("top_accounts").as_arr().unwrap()[0].as_str(), Some("t01"));
}

#[test]
fn script_table_rates() {
    let runs = vec![
        ("fishing".to_string(), "failed".to_string(), 3),
        ("fishing".to_string(), "goal_reached".to_string(), 5),
        ("fishing".to_string(), "blocked".to_string(), 2),
        ("woodcut".to_string(), "budget_exhausted".to_string(), 4),
    ];
    let table = script_table(&sample(), &runs);
    assert_eq!(table[0].get("script").as_str(), Some("fishing"));
    assert_eq!(table[0].get("runs").as_i64(), Some(10));
    assert_eq!(table[0].get("failure_rate").as_f64(), Some(0.3));
    assert_eq!(table[0].get("block_rate").as_f64(), Some(0.2));
    assert_eq!(table[0].get("errors").as_i64(), Some(1));
    let woodcut = table.iter().find(|line| line.get("script").as_str() == Some("woodcut")).unwrap();
    assert_eq!(woodcut.get("failure_rate").as_f64(), Some(0.0));
}

#[test]
fn summary_kpis() {
    let rows = sample();
    let runs = vec![("fishing".to_string(), "failed".to_string(), 1), ("fishing".to_string(), "goal_reached".to_string(), 3)];
    let body = lifescape_json::object(summarize(&SummaryInput {
        rows: &rows,
        since: 0,
        until: 2 * 3_600 * 5,
        previous_total: 4,
        runs: &runs,
        deals: (3, 1),
    }));
    let kpis = body.get("kpis");
    assert_eq!(kpis.get("total").as_i64(), Some(5));
    assert_eq!(kpis.get("per_hour").as_f64(), Some(0.5));
    assert_eq!(kpis.get("trend_pct").as_f64(), Some(25.0));
    assert_eq!(kpis.get("accounts").as_i64(), Some(3), "peer counts as hit");
    assert_eq!(kpis.get("signatures").as_i64(), Some(3));
    assert_eq!(kpis.get("run_failure_rate").as_f64(), Some(0.25));
    assert_eq!(kpis.get("deal_failure_rate").as_f64(), Some(0.25));
    assert_eq!(kpis.get("nav").as_i64(), Some(3));
    assert_eq!(kpis.get("brain").as_i64(), Some(0));
    assert_eq!(body.get("hourly").as_arr().map(Vec::len), Some(10));
    let by_zone = body.get("by_zone").as_arr().unwrap();
    assert_eq!(by_zone[0].get("key").as_str(), Some("Varrock"));
    assert_eq!(by_zone[0].get("count").as_i64(), Some(3));
    assert_eq!(body.get("by_kind").as_arr().unwrap()[0].get("key").as_str(), Some("nav/NoRoute"));
}

#[test]
fn filter_matching_and_options() {
    let rows = sample();
    let by_peer = ErrorFilter { account: "t03".to_string(), ..ErrorFilter::default() };
    assert_eq!(rows.iter().filter(|row| row_matches(row, &by_peer)).count(), 1);
    let windowed = ErrorFilter { since: 3_650, until: 7_400, source: "nav".to_string(), ..ErrorFilter::default() };
    assert_eq!(rows.iter().filter(|row| row_matches(row, &windowed)).count(), 2);
    let options = filter_options(&rows);
    assert_eq!(options.get("accounts").as_arr().map(Vec::len), Some(3));
    assert_eq!(options.get("sources").as_arr().map(Vec::len), Some(3));
    assert_eq!(options.get("zones").as_arr().unwrap()[0].as_str(), Some("Lumbridge"));
}
