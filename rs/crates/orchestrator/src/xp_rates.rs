#![forbid(unsafe_code)]
//! XP / tick page (user 2026-11-22, `/xp`): pure model, report rows in,
//! one JSON snapshot out. Gathering, the 30-minute schedule and the
//! cache live in `xp_service`; the endpoint only serves the snapshot.
//!
//! Definitions (every rate is a ratio of sums, never a mean of rates):
//! - `fleet_per_tick` of a skill in a bucket = the fleet's XP in that
//!   skill / the fleet's work ticks in the bucket (all reports): the
//!   skill's contribution per tick of fleet work; the skills add up;
//! - `efficiency` of a skill = its XP / the work ticks of the reports
//!   that gave XP in it: XP per tick spent on that skill's activities.
//!   A combat report counts its ticks in every combat skill it fed, so
//!   skill ticks overlap (their shares can exceed 100 % together);
//! - XP is the display XP the task reports record (not the server's
//!   tenfold unit);
//! - staff and private accounts (`economy_guard::excluded_accounts`) and
//!   reports without work ticks never count.

use std::collections::{BTreeMap, BTreeSet};

use lifescape_json::{Value, object, str_value};
use lifescape_store::{BucketWork, SkillBucket};

/// Snapshot period, like the hiscores.
pub const INTERVAL_SECONDS: i64 = crate::hiscores::INTERVAL_SECONDS;
/// First snapshot after a daemon start.
pub const START_DELAY_SECONDS: i64 = crate::hiscores::START_DELAY_SECONDS;
/// Bucket width of the chart and of the store sums (periods align on it).
pub const BUCKET_SECONDS: i64 = 3600;
/// Chart slots: 7 days of hours.
pub const SLOT_COUNT: usize = 168;
/// A method needs this many work ticks in a period to be ranked.
pub const MIN_METHOD_TICKS: i64 = 1000;
/// Top methods listed across all skills.
pub const TOP_METHODS: usize = 15;
/// Table periods: (key, length in seconds); length 0 = all the stored
/// history (no previous period to compare with).
pub const PERIODS: [(&str, i64); 3] = [("24h", 24 * 3600), ("7d", 7 * 24 * 3600), ("all", 0)];
/// A skill below this share of the fleet's work time is flagged idle.
pub const IDLE_SHARE: f64 = 0.01;

/// Free-to-play skills, in the hiscores order (stat names).
pub const F2P_SKILLS: [&str; 15] = [
    "attack", "defence", "strength", "hitpoints", "ranged", "prayer", "magic", "cooking", "woodcutting", "firemaking", "fishing", "crafting", "smithing", "mining",
    "runecraft",
];

/// Oldest clock a snapshot at `now` reads: the start of the previous
/// 7 day period (it holds the chart's first slot).
pub fn since_clock(now: i64) -> i64 {
    period_start(now, 7 * 24 * 3600) - 7 * 24 * 3600
}

/// Start of the period of `length` seconds ending at `now`, aligned down
/// to the bucket so every row shares one boundary; 0 for all-time.
pub fn period_start(now: i64, length: i64) -> i64 {
    if length <= 0 {
        return 0;
    }
    ((now - length) / BUCKET_SECONDS) * BUCKET_SECONDS
}

/// The rows of the history the "all" period reads: the store's whole
/// history folded into bucket 0, or the windowed rows themselves when the
/// window already holds all of it.
pub struct History<'a> {
    pub rows: &'a [SkillBucket],
    pub buckets: &'a [BucketWork],
    /// Clock of the oldest report with work ticks (span of the "all" rates).
    pub oldest_clock: i64,
}

/// Ratio of two sums, None without a denominator.
fn ratio(numerator: i64, denominator: i64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn number(value: Option<f64>, decimals: i32) -> Value {
    let scale = 10f64.powi(decimals);
    value.filter(|found| found.is_finite()).map_or(Value::Null, |found| Value::Float((found * scale).round() / scale))
}

/// Sums of one slice of reports for one skill.
#[derive(Default, Clone)]
struct Sums {
    xp: i64,
    ticks: i64,
    runs: i64,
    bots: BTreeSet<String>,
}

impl Sums {
    fn add(&mut self, row: &SkillBucket) {
        self.xp += row.xp;
        self.ticks += row.work_ticks;
        self.runs += row.runs;
        self.bots.insert(row.account.to_lowercase());
    }

    fn rate(&self) -> Option<f64> {
        ratio(self.xp, self.ticks)
    }
}

fn counts(account: &str, excluded: &BTreeSet<String>) -> bool {
    !excluded.contains(&account.to_lowercase())
}

/// Rows of the clock range `[start, end)`, staff left out.
fn in_range<'a>(rows: &'a [SkillBucket], start: i64, end: i64, excluded: &'a BTreeSet<String>) -> impl Iterator<Item = &'a SkillBucket> {
    rows.iter().filter(move |row| row.bucket_start >= start && row.bucket_start < end && counts(&row.account, excluded))
}

fn fleet_ticks(buckets: &[BucketWork], start: i64, end: i64, excluded: &BTreeSet<String>) -> i64 {
    buckets.iter().filter(|bucket| bucket.bucket_start >= start && bucket.bucket_start < end && counts(&bucket.account, excluded)).map(|bucket| bucket.work_ticks).sum()
}

type MethodSums = BTreeMap<(String, String), Sums>;

/// Per skill sums and per (skill, method) sums of one range.
fn sums_of(rows: &[SkillBucket], start: i64, end: i64, excluded: &BTreeSet<String>) -> (BTreeMap<String, Sums>, MethodSums) {
    let mut skills: BTreeMap<String, Sums> = BTreeMap::new();
    let mut methods: MethodSums = BTreeMap::new();
    for row in in_range(rows, start, end, excluded) {
        skills.entry(row.skill.clone()).or_default().add(row);
        if !row.planner_method.is_empty() {
            methods.entry((row.skill.clone(), row.planner_method.clone())).or_default().add(row);
        }
    }
    (skills, methods)
}

/// Best method of a skill by XP per tick, among those with enough ticks.
fn best_method<'a>(methods: &'a MethodSums, skill: &str) -> Option<(&'a str, &'a Sums)> {
    methods
        .iter()
        .filter(|((method_skill, _), sums)| method_skill == skill && sums.ticks >= MIN_METHOD_TICKS)
        .map(|((_, method), sums)| (method.as_str(), sums))
        .max_by(|left, right| left.1.rate().unwrap_or(0.0).total_cmp(&right.1.rate().unwrap_or(0.0)).then_with(|| right.0.cmp(left.0)))
}

fn method_value(skill: &str, method: &str, sums: &Sums) -> Value {
    object(vec![
        ("skill", str_value(skill)),
        ("method", str_value(method)),
        ("xp_per_tick", number(sums.rate(), 3)),
        ("ticks", Value::Int(sums.ticks)),
        ("runs", Value::Int(sums.runs)),
        ("bots", Value::Int(sums.bots.len() as i64)),
    ])
}

/// Percent change of `current` against `previous`, None without a base.
pub fn change_percent(current: Option<f64>, previous: Option<f64>) -> Option<f64> {
    let (current, previous) = (current?, previous?);
    (previous > 0.0).then(|| (current - previous) / previous * 100.0)
}

/// Skill table rows, fleet totals per period and the top methods (24 h).
fn period_table(rows: &[SkillBucket], buckets: &[BucketWork], history: &History, now: i64, excluded: &BTreeSet<String>) -> (Value, Value, Value) {
    let mut table: BTreeMap<&str, Vec<(&str, Value)>> = F2P_SKILLS.iter().map(|skill| (*skill, Vec::new())).collect();
    let mut fleet = Vec::new();
    let mut top_methods: Vec<(String, String, Sums)> = Vec::new();
    for (key, length) in PERIODS {
        let start = period_start(now, length);
        let (source_rows, source_buckets) = if length > 0 { (rows, buckets) } else { (history.rows, history.buckets) };
        let fleet_total = fleet_ticks(source_buckets, start, now + 1, excluded);
        let (skills, methods) = sums_of(source_rows, start, now + 1, excluded);
        let previous_skills = if length > 0 { sums_of(rows, start - length, start, excluded).0 } else { BTreeMap::new() };
        let span = if length > 0 { length } else { (now - history.oldest_clock).max(BUCKET_SECONDS) };
        let hours = span as f64 / 3600.0;
        let mut fleet_xp = 0;
        for skill in F2P_SKILLS {
            let sums = skills.get(skill).cloned().unwrap_or_default();
            let previous_rate = previous_skills.get(skill).and_then(Sums::rate);
            fleet_xp += sums.xp;
            let share = ratio(sums.ticks, fleet_total);
            let best = best_method(&methods, skill).map(|(method, found)| method_value(skill, method, found)).unwrap_or(Value::Null);
            table.entry(skill).or_default().push((
                key,
                object(vec![
                    ("xp", Value::Int(sums.xp)),
                    ("work_ticks", Value::Int(sums.ticks)),
                    ("efficiency", number(sums.rate(), 3)),
                    ("fleet_per_tick", number(ratio(sums.xp, fleet_total), 3)),
                    ("xp_per_hour", number(Some(sums.xp as f64 / hours), 0)),
                    ("share", number(share, 4)),
                    ("idle", Value::Bool(share.unwrap_or(0.0) < IDLE_SHARE)),
                    ("bots", Value::Int(sums.bots.len() as i64)),
                    ("previous_efficiency", number(previous_rate, 3)),
                    ("change_percent", number(change_percent(sums.rate(), previous_rate), 1)),
                    ("best_method", best),
                ]),
            ));
        }
        if key == "24h" {
            for ((skill, method), sums) in &methods {
                if F2P_SKILLS.contains(&skill.as_str()) && sums.ticks >= MIN_METHOD_TICKS {
                    top_methods.push((skill.clone(), method.clone(), sums.clone()));
                }
            }
        }
        let active_bots: BTreeSet<String> = in_range(source_rows, start, now + 1, excluded).map(|row| row.account.to_lowercase()).collect();
        fleet.push((
            key,
            object(vec![
                ("work_ticks", Value::Int(fleet_total)),
                ("xp", Value::Int(fleet_xp)),
                ("xp_per_tick", number(ratio(fleet_xp, fleet_total), 3)),
                ("bots", Value::Int(active_bots.len() as i64)),
            ]),
        ));
    }
    top_methods.sort_by(|left, right| right.2.rate().unwrap_or(0.0).total_cmp(&left.2.rate().unwrap_or(0.0)).then_with(|| left.1.cmp(&right.1)));
    let top = top_methods.iter().take(TOP_METHODS).map(|(skill, method, sums)| method_value(skill, method, sums)).collect();
    let skill_rows = F2P_SKILLS
        .iter()
        .map(|skill| object(vec![("skill", str_value(skill)), ("periods", object(table.remove(skill).unwrap_or_default()))]))
        .collect();
    (Value::Arr(skill_rows), object(fleet), Value::Arr(top))
}

/// Chart series over the `SLOT_COUNT` hourly slots ending on the one
/// holding `now`: per skill the contribution per fleet tick and the
/// efficiency, null where the denominator is empty.
fn chart(rows: &[SkillBucket], buckets: &[BucketWork], now: i64, excluded: &BTreeSet<String>) -> (Vec<i64>, Vec<i64>, Value) {
    let last = (now / BUCKET_SECONDS) * BUCKET_SECONDS;
    let slots: Vec<i64> = (0..SLOT_COUNT as i64).map(|index| last - (SLOT_COUNT as i64 - 1 - index) * BUCKET_SECONDS).collect();
    let first = slots[0];
    let slot_of = |clock: i64| -> Option<usize> {
        let offset = clock - first;
        (offset >= 0).then(|| (offset / BUCKET_SECONDS) as usize).filter(|slot| *slot < SLOT_COUNT)
    };
    let mut fleet = vec![0i64; SLOT_COUNT];
    for bucket in buckets.iter().filter(|bucket| counts(&bucket.account, excluded)) {
        if let Some(slot) = slot_of(bucket.bucket_start) {
            fleet[slot] += bucket.work_ticks;
        }
    }
    let mut xp: BTreeMap<&str, Vec<i64>> = F2P_SKILLS.iter().map(|skill| (*skill, vec![0; SLOT_COUNT])).collect();
    let mut ticks = xp.clone();
    for row in rows.iter().filter(|row| counts(&row.account, excluded)) {
        let Some(slot) = slot_of(row.bucket_start) else { continue };
        if let Some(values) = xp.get_mut(row.skill.as_str()) {
            values[slot] += row.xp;
        }
        if let Some(values) = ticks.get_mut(row.skill.as_str()) {
            values[slot] += row.work_ticks;
        }
    }
    let series = F2P_SKILLS
        .iter()
        .map(|skill| {
            let per_fleet: Vec<Value> = (0..SLOT_COUNT).map(|slot| number(ratio(xp[skill][slot], fleet[slot]), 3)).collect();
            let efficiency: Vec<Value> = (0..SLOT_COUNT).map(|slot| number(ratio(xp[skill][slot], ticks[skill][slot]), 3)).collect();
            (*skill, object(vec![("fleet_per_tick", Value::Arr(per_fleet)), ("efficiency", Value::Arr(efficiency))]))
        })
        .collect();
    (slots, fleet, object(series))
}

/// The page's snapshot.
pub fn build(rows: &[SkillBucket], buckets: &[BucketWork], history: &History, now: i64, next_at: i64, excluded: &BTreeSet<String>) -> Value {
    let (slots, fleet, series) = chart(rows, buckets, now, excluded);
    let (table, fleet_periods, top_methods) = period_table(rows, buckets, history, now, excluded);
    object(vec![
        ("ok", Value::Bool(true)),
        ("generated_at", Value::Int(now)),
        ("next_at", Value::Int(next_at)),
        ("bucket_seconds", Value::Int(BUCKET_SECONDS)),
        ("skills", Value::Arr(F2P_SKILLS.iter().map(|skill| str_value(skill)).collect())),
        ("slots", Value::Arr(slots.into_iter().map(Value::Int).collect())),
        ("fleet_ticks", Value::Arr(fleet.into_iter().map(Value::Int).collect())),
        ("series", series),
        ("periods", Value::Arr(PERIODS.iter().map(|(key, length)| object(vec![("key", str_value(key)), ("seconds", Value::Int(*length))])).collect())),
        ("table", table),
        ("fleet", fleet_periods),
        ("top_methods", top_methods),
        ("min_method_ticks", Value::Int(MIN_METHOD_TICKS)),
        ("idle_share", Value::Float(IDLE_SHARE)),
    ])
}
