use std::collections::BTreeSet;

use lifescape_json::Value;
use lifescape_store::{BucketWork, SkillBucket};

use crate::xp_rates::{build, change_percent, history_bucket_seconds, period_start, since_clock, ChartHistory, History, F2P_SKILLS, SLOT_COUNT};

const NOW: i64 = 1_800_000_000 + 1800;
const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

fn row(account: &str, method: &str, bucket_start: i64, ticks: i64, skill: &str, xp: i64) -> SkillBucket {
    SkillBucket { bucket_start, account: account.to_string(), planner_method: method.to_string(), skill: skill.to_string(), xp, work_ticks: ticks, runs: 1 }
}

fn work(bucket_start: i64, account: &str, ticks: i64) -> BucketWork {
    BucketWork { bucket_start, account: account.to_string(), work_ticks: ticks }
}

fn excluded() -> BTreeSet<String> {
    ["admin", "admin2", "bite"].iter().map(|name| name.to_string()).collect()
}

fn snapshot_of(rows: &[SkillBucket], buckets: &[BucketWork]) -> Value {
    let history = History { rows, buckets, oldest_clock: NOW - 3 * DAY };
    let chart_history = ChartHistory { rows, buckets, bucket_seconds: HOUR };
    build(rows, buckets, &history, &chart_history, NOW, NOW + 1800, &excluded())
}

fn skill_row<'a>(snapshot: &'a Value, skill: &str) -> &'a Value {
    snapshot.get("table").as_arr().unwrap().iter().find(|entry| entry.get("skill").as_str() == Some(skill)).unwrap()
}

#[test]
fn rates_are_ratios_of_sums_and_staff_never_counts() {
    let recent = (NOW / HOUR) * HOUR - HOUR;
    // 3000 xp / 1000 ticks and 100 xp / 1000 ticks: ratio of sums 1.55,
    // a mean of the two rates would give 1.65.
    let rows = vec![
        row("ann", "chop:willow", recent, 1000, "woodcutting", 3000),
        row("bob", "chop:willow", recent, 1000, "woodcutting", 100),
        row("admin", "chop:willow", recent, 5000, "woodcutting", 99_999),
        row("BITE", "chop:willow", recent, 5000, "woodcutting", 99_999),
        row("ann", "chop:willow", recent, 1000, "fletching", 500),
    ];
    let buckets = vec![work(recent, "ann", 1000), work(recent, "bob", 1000), work(recent, "admin", 5000), work(recent, "bite", 5000)];
    let snapshot = snapshot_of(&rows, &buckets);
    for key in ["24h", "7d", "all"] {
        let period = skill_row(&snapshot, "woodcutting").get("periods").get(key);
        assert_eq!(period.get("xp").as_i64(), Some(3100), "{key}");
        assert_eq!(period.get("work_ticks").as_i64(), Some(2000), "{key}");
        assert_eq!(period.get("efficiency").as_f64(), Some(9300.0), "{key}");
        assert_eq!(period.get("fleet_rate").as_f64(), Some(9300.0), "{key}");
        assert_eq!(period.get("share").as_f64(), Some(1.0), "{key}");
        assert_eq!(period.get("bots").as_i64(), Some(2), "{key}");
    }
    let day = skill_row(&snapshot, "woodcutting").get("periods").get("24h");
    assert_eq!(day.get("best_method").get("method").as_str(), Some("chop:willow"));
    assert_eq!(snapshot.get("fleet").get("24h").get("work_ticks").as_i64(), Some(2000));
    assert!(snapshot.get("table").as_arr().unwrap().iter().all(|entry| entry.get("skill").as_str() != Some("fletching")));
}

#[test]
fn chart_is_hourly_over_seven_days_with_null_gaps() {
    let slot_start = (NOW / HOUR) * HOUR;
    let rows = vec![row("ann", "m", slot_start, 500, "mining", 250), row("ann", "m", slot_start - HOUR, 1000, "mining", 50)];
    let buckets = vec![work(slot_start, "ann", 500), work(slot_start - HOUR, "ann", 2000)];
    let snapshot = snapshot_of(&rows, &buckets);
    assert_eq!(SLOT_COUNT, 168);
    assert_eq!(snapshot.get("slots").as_arr().unwrap().len(), SLOT_COUNT);
    let mining = snapshot.get("series").get("mining");
    let per_fleet = mining.get("fleet_rate").as_arr().unwrap();
    assert_eq!(per_fleet[SLOT_COUNT - 1].as_f64(), Some(3000.0));
    assert_eq!(per_fleet[SLOT_COUNT - 2].as_f64(), Some(150.0));
    assert!(per_fleet[0].is_null());
    assert_eq!(mining.get("efficiency").as_arr().unwrap()[SLOT_COUNT - 2].as_f64(), Some(300.0));
    assert_eq!(snapshot.get("skills").as_arr().unwrap().len(), F2P_SKILLS.len());
}

#[test]
fn change_compares_with_the_previous_period_and_best_method_needs_ticks() {
    let start = period_start(NOW, DAY);
    let rows = vec![
        row("ann", "slow", NOW - HOUR, 2000, "mining", 1000),
        row("ann", "fast", NOW - HOUR, 500, "mining", 2000),
        row("ann", "prev", start - HOUR, 2000, "mining", 500),
    ];
    let snapshot = snapshot_of(&rows, &[work(NOW - HOUR, "ann", 2500)]);
    let day = skill_row(&snapshot, "mining").get("periods").get("24h");
    // fast has 500 ticks < the minimum: slow is ranked.
    assert_eq!(day.get("best_method").get("method").as_str(), Some("slow"));
    // 3000 / 2500 = 1.2 against 500 / 2000 = 0.25.
    assert_eq!(day.get("change_percent").as_f64(), Some(380.0));
    // The all-time period has no previous period.
    assert!(skill_row(&snapshot, "mining").get("periods").get("all").get("change_percent").is_null());
    assert_eq!(change_percent(Some(1.0), None), None);
    assert_eq!(change_percent(Some(1.0), Some(0.0)), None);
}

#[test]
fn all_time_reads_its_own_history_and_idle_skills_are_flagged() {
    let old = NOW - 30 * DAY;
    let rows = vec![row("ann", "m", NOW - HOUR, 1000, "mining", 100)];
    let all_rows = vec![row("ann", "m", 0, 41_000, "mining", 4100)];
    let buckets = vec![work(NOW - HOUR, "ann", 1000)];
    let all_buckets = vec![work(0, "ann", 41_000)];
    let history = History { rows: &all_rows, buckets: &all_buckets, oldest_clock: old };
    let chart_history = ChartHistory { rows: &rows, buckets: &buckets, bucket_seconds: HOUR };
    let snapshot = build(&rows, &buckets, &history, &chart_history, NOW, NOW + 1800, &excluded());
    let all = skill_row(&snapshot, "mining").get("periods").get("all");
    assert_eq!(all.get("xp").as_i64(), Some(4100));
    assert_eq!(all.get("efficiency").as_f64(), Some(600.0));
    assert_eq!(skill_row(&snapshot, "mining").get("periods").get("7d").get("xp").as_i64(), Some(100));
    let prayer = skill_row(&snapshot, "prayer").get("periods").get("7d");
    assert_eq!(prayer.get("idle").as_bool(), Some(true));
    assert!(prayer.get("efficiency").is_null());
    assert!(since_clock(NOW) <= period_start(NOW, 7 * DAY) - 7 * DAY);
    assert!(since_clock(NOW) <= NOW - 167 * HOUR);
}

#[test]
fn rates_are_per_6000_ticks_in_cards_tables_and_top_methods() {
    let recent = (NOW / HOUR) * HOUR - HOUR;
    // 150 xp over 600 ticks = 0.25 per tick = 1500 per 6000 ticks.
    let rows = vec![row("ann", "chop:oak", recent, 1000, "woodcutting", 250), row("ann", "chop:oak", recent, 1000, "woodcutting", 50)];
    let snapshot = snapshot_of(&rows, &[work(recent, "ann", 2000)]);
    assert_eq!(snapshot.get("rate_ticks").as_i64(), Some(6000));
    let day = skill_row(&snapshot, "woodcutting").get("periods").get("24h");
    assert_eq!(day.get("efficiency").as_f64(), Some(900.0));
    assert_eq!(day.get("fleet_rate").as_f64(), Some(900.0));
    assert_eq!(snapshot.get("fleet").get("24h").get("xp_rate").as_f64(), Some(900.0));
    assert_eq!(snapshot.get("top_methods").as_arr().unwrap()[0].get("xp_rate").as_f64(), Some(900.0));
    assert_eq!(day.get("best_method").get("xp_rate").as_f64(), Some(900.0));
}

#[test]
fn history_bucket_width_grows_with_the_span() {
    assert_eq!(history_bucket_seconds(3 * DAY), HOUR);
    assert_eq!(history_bucket_seconds(7 * DAY), HOUR);
    assert_eq!(history_bucket_seconds(7 * DAY + 1), 6 * HOUR);
    assert_eq!(history_bucket_seconds(45 * DAY), 6 * HOUR);
    assert_eq!(history_bucket_seconds(45 * DAY + 1), DAY);
}

#[test]
fn all_history_chart_sums_per_bucket_from_the_oldest_report() {
    let width = 6 * HOUR;
    let oldest = NOW - 20 * DAY;
    let first = (oldest / width) * width;
    let last = (NOW / width) * width;
    // Two buckets of 6 h: xp 300 / ticks 100 and xp 100 / ticks 100 are
    // summed (400 / 200 * 6000 = 12000), not averaged per bucket.
    let all_rows = vec![row("ann", "m", first, 100, "mining", 300), row("ann", "m", last, 100, "mining", 100), row("admin", "m", first, 100, "mining", 9999)];
    let all_buckets = vec![work(first, "ann", 100), work(last, "ann", 100), work(first, "admin", 100)];
    let week_rows = vec![row("ann", "m", NOW - HOUR, 100, "mining", 100)];
    let history = History { rows: &all_rows, buckets: &all_buckets, oldest_clock: oldest };
    let chart_history = ChartHistory { rows: &all_rows, buckets: &all_buckets, bucket_seconds: width };
    let snapshot = build(&week_rows, &[work(NOW - HOUR, "ann", 100)], &history, &chart_history, NOW, NOW + 1800, &excluded());
    let chart = snapshot.get("history_chart");
    assert_eq!(chart.get("bucket_seconds").as_i64(), Some(width));
    let slots = chart.get("slots").as_arr().unwrap();
    assert_eq!(slots[0].as_i64(), Some(first));
    assert_eq!(slots[slots.len() - 1].as_i64(), Some(last));
    assert_eq!(slots.len() as i64, (last - first) / width + 1);
    let mining = chart.get("series").get("mining");
    let efficiency = mining.get("efficiency").as_arr().unwrap();
    assert_eq!(efficiency[0].as_f64(), Some(18000.0));
    assert_eq!(efficiency[efficiency.len() - 1].as_f64(), Some(6000.0));
    assert!(efficiency[1].is_null());
    assert_eq!(chart.get("fleet_ticks").as_arr().unwrap()[0].as_i64(), Some(100));
    // The hourly 7 day chart is untouched.
    assert_eq!(snapshot.get("slots").as_arr().unwrap().len(), SLOT_COUNT);
}

#[test]
fn fleet_chart_series_hold_raw_xp_and_xp_per_6000_ticks() {
    let slot_start = (NOW / HOUR) * HOUR;
    // 250 + 50 xp in two skills over 600 fleet ticks (staff left out):
    // 300 / 600 * 6000 = 3000.
    let rows = vec![
        row("ann", "m", slot_start, 400, "mining", 250),
        row("ann", "m", slot_start, 200, "attack", 50),
        row("admin", "m", slot_start, 200, "attack", 9999),
    ];
    let buckets = vec![work(slot_start, "ann", 600), work(slot_start, "admin", 5000)];
    let snapshot = snapshot_of(&rows, &buckets);
    let fleet_xp = snapshot.get("fleet_xp").as_arr().unwrap();
    let fleet_rate = snapshot.get("fleet_rate").as_arr().unwrap();
    assert_eq!(fleet_xp.len(), SLOT_COUNT);
    assert_eq!(fleet_xp[SLOT_COUNT - 1].as_i64(), Some(300));
    assert_eq!(fleet_rate[SLOT_COUNT - 1].as_f64(), Some(3000.0));
    assert_eq!(fleet_xp[0].as_i64(), Some(0));
    assert!(fleet_rate[0].is_null());
    let history = snapshot.get("history_chart");
    assert_eq!(history.get("fleet_xp").as_arr().unwrap().iter().filter_map(Value::as_i64).sum::<i64>(), 300);
    assert!(history.get("fleet_rate").as_arr().unwrap().iter().any(|value| value.as_f64() == Some(3000.0)));
}
