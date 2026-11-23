use super::*;

fn sample(qty: i64, ticks: i64) -> RunSample {
    RunSample {
        work_ticks: ticks,
        pause_ticks: 0,
        flows: vec![("Logs".to_string(), "product".to_string(), qty)],
        units: vec![("product".to_string(), "Logs".to_string(), qty)],
        clock: 1,
    }
}

fn logs_rate(bucket: &Bucket) -> f64 {
    bucket.flow_rates().iter().find(|(item, _, _)| item == "Logs").map(|(_, _, rate)| *rate).unwrap_or(f64::NAN)
}

fn key(site: &str) -> YieldKey {
    YieldKey { method: "gather:woodcut/logs".to_string(), style: String::new(), site: site.to_string(), tool: "bronze_axe".to_string(), band: 1 }
}

/// Σ quantities / Σ work ticks, never a mean of ratios: a 100-tick
/// lucky run of 20 logs (1200 / 6000 as a ratio) barely moves a bucket
/// of long runs.
#[test]
fn estimate_is_sum_over_sum() {
    let mut bucket = Bucket::default();
    bucket.add(&sample(300, 6000));
    bucket.add(&sample(300, 6000));
    let decayed = 300.0 * decay_factor(6000) + 300.0;
    let ticks = 6000.0 * decay_factor(6000) + 6000.0;
    assert!((logs_rate(&bucket) - decayed / ticks * 6000.0).abs() < 1e-9);
    assert!((logs_rate(&bucket) - 300.0).abs() < 1e-9, "same pace, same rate");
    bucket.add(&sample(20, 100));
    assert!(logs_rate(&bucket) < 320.0, "a short lucky run weighs a third and its own ticks: {}", logs_rate(&bucket));
    assert_eq!(bucket.runs, 3);
}

/// D7: the old pace halves after one half-life of newer work.
#[test]
fn decay_follows_cumulated_work_ticks() {
    let mut bucket = Bucket::default();
    bucket.add(&sample(600, 6000));
    for _ in 0..10 {
        bucket.add(&sample(300, 6000));
    }
    // The first run's sums weigh 0.5^(60000 / 60000) = 0.5 now.
    let old = 600.0 * 0.5;
    let new: f64 = (0..10).map(|index| 300.0 * decay_factor(6000 * index)).sum();
    let ticks: f64 = 6000.0 * 0.5 + (0..10).map(|index| 6000.0 * decay_factor(6000 * index)).sum::<f64>();
    assert!((logs_rate(&bucket) - (old + new) / ticks * 6000.0).abs() < 1e-6);
    assert!(logs_rate(&bucket) < 330.0);
    assert!((run_weight(150) - 0.5).abs() < 1e-12, "a 150-tick run weighs half");
}

/// Memory per bucket stays bounded whatever the number of runs: sums,
/// never samples.
#[test]
fn bucket_memory_is_bounded() {
    let mut book = YieldBook::default();
    for index in 0..5000 {
        book.record(&key("3100,3200,0"), &sample(100 + index % 7, 1000));
    }
    let bucket = book.get(&key("3100,3200,0")).expect("bucket");
    assert_eq!(bucket.runs, 5000);
    assert_eq!((bucket.flows.len(), bucket.units.len()), (1, 1));
    assert_eq!(book.buckets.len(), 3, "the key and its two roll-ups");
    for index in 0..(MAX_BUCKETS + 50) {
        let mut sample = sample(10, 1000);
        sample.clock = index as i64;
        book.record(&YieldKey { band: index as i64, ..key("1,1,0") }, &sample);
    }
    assert!(book.buckets.len() <= MAX_BUCKETS);
}

/// Failed and blocked runs stay out of the rate, in the failure rate.
#[test]
fn failures_stay_out_of_the_rate() {
    let mut book = YieldBook::default();
    book.record(&key("1,1,0"), &sample(300, 6000));
    book.record_failure(&key("1,1,0"), 600);
    book.record_failure(&key("1,1,0"), 600);
    let bucket = book.get(&key("1,1,0")).expect("bucket");
    assert!((logs_rate(bucket) - 300.0).abs() < 1e-9);
    assert_eq!(bucket.runs, 1);
    assert!(bucket.failure_rate() > 0.6, "{}", bucket.failure_rate());
    assert!(book.get(&key("1,1,0").any_site_tool()).is_some_and(|rollup| rollup.failed > 0.0));
}

/// Two sites roll up into the all-sites key with their own sums.
#[test]
fn rollups_merge_sites() {
    let mut book = YieldBook::default();
    book.record(&key("1,1,0"), &sample(600, 6000));
    book.record(&key("2,2,0"), &sample(200, 6000));
    let merged = book.get(&key("1,1,0").any_site()).expect("rollup");
    assert_eq!(merged.runs, 2);
    assert!(logs_rate(merged) > 200.0 && logs_rate(merged) < 600.0);
    assert!(merged.dispersion() > 0.0);
}
