//! Lot 4.3 / 4.4: the heartbeat channel wakes the brain. The report flag is
//! taken once whoever asks (no double fold), the mailbox keeps one pass per
//! account, and a wake never leaves a report in flight behind it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lifescape_json::{object, str_value, Value};

use crate::brain_raw_world_tests::IdleFleet;
use crate::jobs::Job;

fn pending(job: &Job, n: i64) {
    let mut inner = job.inner.lock().unwrap();
    inner.last_report = Some(object(vec![("stop", str_value("goal_reached")), ("n", Value::Int(n))]));
    inner.report_pending = true;
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !condition() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The heartbeat reader and the supervisor drain race for 600 reports:
/// every report is folded exactly once, by one of them, in no case twice
/// or lost. (The supervisor drain is the same `mem::replace` under the job
/// lock that `take_report` does.)
#[test]
fn the_first_consumer_of_the_report_flag_wins_and_nothing_folds_twice() {
    let job = Arc::new(Job::new("wake-race"));
    let taken: Arc<Vec<AtomicU64>> = Arc::new((0..600).map(|_| AtomicU64::new(0)).collect());
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut consumers = Vec::new();
    for style in 0..6 {
        let (job, taken, stop) = (job.clone(), taken.clone(), stop.clone());
        consumers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let report = if style % 2 == 0 {
                    job.take_report()
                } else {
                    // The supervisor's drain, as written in `supervise_all`.
                    match job.inner.try_lock() {
                        Ok(mut inner) => {
                            let flagged = std::mem::replace(&mut inner.report_pending, false);
                            if flagged { inner.last_report.clone() } else { None }
                        }
                        Err(_) => None,
                    }
                };
                if let Some(report) = report {
                    taken[report.get("n").as_i64().unwrap() as usize].fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for n in 0..600 {
        // The producer waits for the previous report to be consumed (one
        // `last_report` slot: a second push before the drain overwrites).
        wait_until(|| !job.inner.lock().unwrap().report_pending);
        pending(&job, n);
    }
    wait_until(|| !job.inner.lock().unwrap().report_pending);
    std::thread::sleep(std::time::Duration::from_millis(50));
    stop.store(true, Ordering::SeqCst);
    for consumer in consumers {
        consumer.join().unwrap();
    }
    for (n, count) in taken.iter().enumerate() {
        assert_eq!(count.load(Ordering::SeqCst), 1, "report {n} folded exactly once");
    }
}

/// A report pushed on the heartbeat channel goes to the mailbox at once;
/// a second wake finds the flag already taken (the supervisor fallback).
#[test]
fn a_pushed_report_is_handed_to_the_brain_once() {
    let fleet = IdleFleet::shared();
    let job = Arc::new(Job::new("wake-report"));
    assert!(!crate::brain::on_report_push(&fleet, &job), "nothing pending: nothing to hand over");
    pending(&job, 1);
    assert!(crate::brain::on_report_push(&fleet, &job));
    assert!(!job.inner.lock().unwrap().report_pending, "flag taken");
    assert!(!crate::brain::on_report_push(&fleet, &job), "the second consumer finds nothing");
    // The shard drops the report of an unknown account and counts it folded.
    wait_until(|| !crate::brain_inflight::in_flight("wake-report"));
    assert!(!crate::brain_inflight::in_flight("wake-report"), "no report left in flight");
}

/// Wakes collapse into the pass already waiting for the account.
#[test]
fn wakes_collapse_into_one_waiting_pass() {
    let fleet = IdleFleet::shared();
    let job = Arc::new(Job::new("wake-collapse"));
    for _ in 0..50 {
        crate::brain::wake(&fleet, &job, crate::brain::WakeReason::Idle);
    }
    wait_until(|| !crate::brain::tick_waiting("wake-collapse"));
    assert!(!crate::brain::tick_waiting("wake-collapse"), "the pass ran and cleared its slot");
}
