#![forbid(unsafe_code)]
//! Per-minute `cpu-profile nav` line (CPU 2026-11-14): the pathfinding
//! service threads' CPU (`plan-pool`, `plan-query`, `plan-wait`,
//! `plan-accept`, read from `/proc/self/task/*/schedstat`) beside the
//! route cache counters of the same minute. One `nav-profile` thread,
//! one wake-up per minute; nothing is logged per request.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lifescape_nav::service::{CacheStats, Service};

/// Summary period (same as the other `cpu-profile` lines).
pub const NAV_WINDOW: Duration = Duration::from_secs(60);

/// CPU nanoseconds per `plan-*` thread group (digits and dashes trimmed:
/// `plan-pool-3` counts in `plan-pool`).
pub fn plan_threads_cpu() -> BTreeMap<String, u64> {
    let mut groups = BTreeMap::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return groups };
    for task in tasks.flatten() {
        let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        let name = comm.trim();
        if !name.starts_with("plan-") {
            continue;
        }
        let group = name.trim_end_matches(|ch: char| ch.is_ascii_digit() || ch == '-').to_string();
        let nanos = std::fs::read_to_string(task.path().join("schedstat"))
            .ok()
            .and_then(|text| text.split_whitespace().next().and_then(|field| field.parse::<u64>().ok()))
            .unwrap_or(0);
        *groups.entry(group).or_insert(0) += nanos;
    }
    groups
}

/// One summary line from two readings (pure: tests feed it directly).
pub fn nav_line(
    span: Duration,
    cpu_before: &BTreeMap<String, u64>,
    cpu_after: &BTreeMap<String, u64>,
    before: &CacheStats,
    after: &CacheStats,
    routes: (usize, usize, usize, usize),
) -> String {
    let mut line = format!("cpu-profile nav ({:.0}s): cpu_ms", span.as_secs_f64());
    let mut total = 0u64;
    for (group, nanos) in cpu_after {
        let used = nanos.saturating_sub(*cpu_before.get(group).unwrap_or(&0));
        total += used;
        line.push_str(&format!(" {group}={:.0}", used as f64 / 1e6));
    }
    line.push_str(&format!(" total={:.0}", total as f64 / 1e6));
    let delta = |pick: fn(&CacheStats) -> u64| pick(after).saturating_sub(pick(before));
    let (exact, dominated, prefix) = (delta(|s| s.exact_hits), delta(|s| s.dominated_hits), delta(|s| s.trunc_hits));
    let (suffix_all, infix, splice) = (delta(|s| s.suffix_hits), delta(|s| s.infix_hits), delta(|s| s.splice_hits));
    let (negative, misses, joins) = (delta(|s| s.negative_hits), delta(|s| s.misses), delta(|s| s.joins));
    let (fast, fast_zero, fast_declined) = (delta(|s| s.fast_hits), delta(|s| s.fast_zero), delta(|s| s.fast_declined));
    let served = exact + dominated + prefix + suffix_all + splice + negative;
    // Short hops answered by the fast path never reach the cache: `hit`
    // stays the cache rate of the other plans, `plans` counts them all.
    let cached = served + misses + joins;
    let plans = cached + fast;
    let rate = if cached > 0 { 100.0 * served as f64 / cached as f64 } else { 0.0 };
    let (entries, negatives, bytes, pockets) = routes;
    line.push_str(&format!(
        " | plans={plans} fast={fast} fast_zero={fast_zero} fast_declined={fast_declined} hit={rate:.0}% exact={exact} dominated={dominated} prefix={prefix} suffix={} infix={infix} splice={splice} negative={negative} miss={misses} joins={joins} | walk_costs={} | routes={entries}+{negatives}neg/{:.1}MiB evict={} pockets={pockets}",
        suffix_all.saturating_sub(infix),
        delta(|s| s.walk_costs),
        bytes as f64 / (1024.0 * 1024.0),
        delta(|s| s.evictions),
    ));
    // Lot N0: what the CPU went on, per kind of request (count / thread
    // CPU / tiles visited), then the `walk_costs` searches that ended
    // without reaching every target. Appended: the fields above keep
    // their place for the readers of the line.
    line.push_str(&format!(" | ops {}", lifescape_nav::nav_meter::ops_segment(&before.ops, &after.ops)));
    line
}

/// Starts the `nav-profile` thread for this service (daemon only).
pub fn start(service: Arc<Service>) {
    let spawned = std::thread::Builder::new().name("nav-profile".to_string()).spawn(move || {
        let mut cpu_before = plan_threads_cpu();
        let mut before = service.stats();
        let mut started = Instant::now();
        loop {
            // Gauge period, not a game wait.
            std::thread::sleep(NAV_WINDOW);
            let cpu_after = plan_threads_cpu();
            let after = service.stats();
            let routes = service.route_stats();
            let occupancy = (routes.entries, routes.negatives, routes.bytes, service.pocket_verdicts());
            crate::logln!("{}", nav_line(started.elapsed(), &cpu_before, &cpu_after, &before, &after, occupancy));
            (cpu_before, before, started) = (cpu_after, after, Instant::now());
        }
    });
    if let Err(error) = spawned {
        eprintln!("nav-profile thread spawn failed ({error}): no cpu-profile nav line");
    }
}

#[cfg(test)]
mod tests {
    use super::nav_line;
    use lifescape_nav::service::CacheStats;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn nav_line_reads_deltas() {
        let cpu_before = BTreeMap::from([("plan-pool".to_string(), 1_000_000u64), ("plan-query".to_string(), 0)]);
        let cpu_after = BTreeMap::from([("plan-pool".to_string(), 51_000_000u64), ("plan-query".to_string(), 2_000_000)]);
        let before = CacheStats::default();
        let after = CacheStats { exact_hits: 6, trunc_hits: 1, suffix_hits: 2, infix_hits: 1, splice_hits: 1, misses: 2, walk_costs: 3, fast_hits: 20, fast_zero: 1, fast_declined: 4, ..CacheStats::default() };
        let line = nav_line(Duration::from_secs(60), &cpu_before, &cpu_after, &before, &after, (10, 2, 1024 * 1024, 5));
        assert!(line.starts_with("cpu-profile nav (60s): cpu_ms plan-pool=50 plan-query=2 total=52"), "{line}");
        assert!(line.contains("plans=32 fast=20 fast_zero=1 fast_declined=4 hit=83% exact=6 dominated=0 prefix=1 suffix=1 infix=1 splice=1 negative=0 miss=2"), "{line}");
        assert!(line.contains("walk_costs=3 | routes=10+2neg/1.0MiB evict=0 pockets=5 | ops "), "{line}");
        assert!(line.ends_with("plan_unreached=0/0v horizon_cut=0"), "{line}");
    }
}
