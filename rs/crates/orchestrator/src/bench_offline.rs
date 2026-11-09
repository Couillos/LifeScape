#![forbid(unsafe_code)]
//! Offline load bench (E15): 300 fake workers replay the measured live
//! traffic against the REAL daemon code and the REAL pathfinding
//! service, with no game server.
//!
//! Each fake worker loops at the measured live pace (~1 plan + 1 push
//! per 700 ms): `plan` ops on 12 canonical Varrock legs (L1-warmed, so
//! the mix mirrors the measured 72 % cache hits) plus snapshot push
//! frames through `Orchestrator::ingest_push`. The report shows
//! plans/s, hit rate, refusals by cause and ingested pushes — the proof
//! the daemon holds 300 bots, or the exact bottleneck when it does not.
//! Live traffic stays capped at 20 bots (D38); this bench is the 300-bot
//! evidence.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lifescape_json::{object, str_value, Value};
use lifescape_nav::plan::Tile;
use lifescape_nav::service::{PlanClient, Service, Snapshot};

use crate::daemon::Orchestrator;
use crate::jobs::Job;

/// Bench traffic shape, mirroring the L0 live measurements.
#[derive(Clone, Debug)]
pub struct BenchConfig {
    /// Fake workers (300 for the gate).
    pub workers: usize,
    /// Bench duration in seconds.
    pub secs: u64,
    /// Distinct legs replayed (12 canonical Varrock legs).
    pub legs: usize,
    /// Pacing per worker iteration (~700 ms live).
    pub pace_ms: u64,
}

impl Default for BenchConfig {
    fn default() -> Self {
        BenchConfig { workers: 300, secs: 15, legs: 12, pace_ms: 700 }
    }
}

/// What the bench measured.
#[derive(Clone, Debug, Default)]
pub struct BenchReport {
    pub workers: usize,
    pub secs: u64,
    pub plans: u64,
    pub plans_per_s: f64,
    pub hits: u64,
    pub trunc_hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub refused_cap: u64,
    pub refused_full: u64,
    pub refused_stale: u64,
    pub unknown: u64,
    pub pushes: u64,
    pub pushes_per_s: f64,
}

impl BenchReport {
    pub fn summary(&self) -> String {
        format!(
            "bench-l2r workers={} secs={} plans={} ({} /s, hits {} + trunc {} / misses {} = {:.0}% cached) refused cap={} full={} stale={} unknown={} pushes={} ({} /s)",
            self.workers,
            self.secs,
            self.plans,
            self.plans_per_s.round(),
            self.hits,
            self.trunc_hits,
            self.misses,
            self.hit_rate * 100.0,
            self.refused_cap,
            self.refused_full,
            self.refused_stale,
            self.unknown,
            self.pushes,
            self.pushes_per_s.round(),
        )
    }
}

/// Twelve canonical Varrock legs (bank <-> fountain <-> gates): the
/// working set the L1 warms on, like live bots converging on Varrock.
fn bench_legs(count: usize) -> Vec<(Tile, Tile)> {
    let spots = [
        (3186, 3440, 0),
        (3213, 3429, 0),
        (3252, 3421, 0),
        (3228, 3214, 0),
        (3224, 3214, 0),
        (3265, 3227, 0),
        (3091, 3242, 0),
        (3203, 3246, 0),
        (3252, 3237, 0),
        (3246, 3438, 0),
        (3210, 3429, 0),
        (3220, 3425, 0),
        (3180, 3430, 0),
        (3200, 3200, 0),
    ];
    let mut legs = Vec::new();
    for index in 0..count.max(1) {
        let from = spots[index % spots.len()];
        let dest = spots[(index * 5 + 3) % spots.len()];
        legs.push((
            Tile { x: from.0, z: from.1, level: from.2 },
            Tile { x: dest.0, z: dest.1, level: dest.2 },
        ));
    }
    legs
}

fn fake_snapshot(account: &str, tick: u64) -> Value {
    object(vec![
        ("type", str_value("snapshot")),
        ("account", str_value(account)),
        (
            "snapshot",
            object(vec![
                ("account", str_value(account)),
                ("ticks", Value::Int(tick as i64)),
                ("updated", Value::Float(crate::jobs::now_secs())),
                (
                    "position",
                    Value::Arr(vec![Value::Int(0), Value::Int(3213), Value::Int(3429)]),
                ),
            ]),
        ),
    ])
}

/// Replay the traffic. Returns the measured report, or a loud error when
/// the snapshot cannot load (engine data missing).
pub fn run_bench(config: &BenchConfig) -> Result<BenchReport, String> {
    let snapshot: Arc<Snapshot> = crate::plan_service::load_snapshot()?;
    let sock = std::env::temp_dir().join(format!("lifescape-bench-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let service = Service::start(snapshot, &sock, lifescape_nav::service::PLAN_THREADS)?;
    let version = service.current_version();
    // Warm the L1 on the working set first (live bots converge too).
    let legs = bench_legs(config.legs);
    let warm = PlanClient::new(service.socket_path(), "bench-warm");
    for (from, dest) in &legs {
        let options = lifescape_nav::plan::NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: std::collections::HashSet::new(),
            excluded_edges: std::collections::HashSet::new(),
        };
        let _ = warm.plan_once(*from, *dest, &options, &version, 10_000);
    }
    let before = service.stats();
    let orch = Arc::new(Orchestrator::new());
    {
        let mut jobs = orch.jobs.lock().unwrap();
        for index in 0..config.workers {
            let account = format!("bench-{index:03}");
            jobs.insert(account.clone(), Arc::new(Job::new(&account)));
        }
    }
    let plans = Arc::new(AtomicU64::new(0));
    let pushes = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + Duration::from_secs(config.secs.max(1));
    let mut handles = Vec::new();
    // One thread per fake worker: live bots loop independently, so the
    // bench must too (shard threads would serialize the pace sleeps and
    // under-drive the daemon 10x). Mostly sleeping, like live workers.
    for worker in 0..config.workers {
        let service_sock = service.socket_path().to_path_buf();
        let version = version.clone();
        let legs = legs.clone();
        let orch = orch.clone();
        let plans = plans.clone();
        let pushes = pushes.clone();
        let pace = config.pace_ms;
        handles.push(
            std::thread::Builder::new()
                .name(format!("bench-fake-{worker}"))
                .spawn(move || {
                    let client = PlanClient::new(&service_sock, &format!("bench-{worker:03}"));
                    let options = lifescape_nav::plan::NavOptions {
                        allow_paid: false,
                        f2p_only: true,
                        satisfied: std::collections::HashSet::new(),
                        excluded_edges: std::collections::HashSet::new(),
                    };
                    let mut tick = 0u64;
                    while Instant::now() < deadline {
                        let (from, dest) = legs[worker % legs.len()];
                        // Distinct accounts per worker (like live bots):
                        // the per-account cap must never fire here.
                        let _ = client.plan_once(from, dest, &options, &version, 10_000);
                        plans.fetch_add(1, Ordering::Relaxed);
                        let account = format!("bench-{worker:03}");
                        let _ = orch.ingest_push(&fake_snapshot(&account, tick));
                        pushes.fetch_add(1, Ordering::Relaxed);
                        tick += 1;
                        std::thread::sleep(Duration::from_millis(pace));
                    }
                })
                .expect("bench worker spawns"),
        );
    }
    for handle in handles {
        let _ = handle.join();
    }
    let after = service.stats();
    let elapsed = config.secs.max(1) as f64;
    let plans_done = plans.load(Ordering::Relaxed);
    let pushes_done = pushes.load(Ordering::Relaxed);
    let hits = after.hits.saturating_sub(before.hits);
    let trunc = after.trunc_hits.saturating_sub(before.trunc_hits);
    let misses = after.misses.saturating_sub(before.misses);
    let _ = std::fs::remove_file(&sock);
    Ok(BenchReport {
        workers: config.workers,
        secs: config.secs,
        plans: plans_done,
        plans_per_s: plans_done as f64 / elapsed,
        hits,
        trunc_hits: trunc,
        misses,
        hit_rate: if plans_done > 0 {
            (hits + trunc) as f64 / plans_done as f64
        } else {
            0.0
        },
        refused_cap: after.refused_cap.saturating_sub(before.refused_cap),
        refused_full: after.refused_full.saturating_sub(before.refused_full),
        refused_stale: after.refused_stale.saturating_sub(before.refused_stale),
        unknown: after.unknown.saturating_sub(before.unknown),
        pushes: pushes_done,
        pushes_per_s: pushes_done as f64 / elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::{run_bench, BenchConfig};

    /// The bench runs end to end on a small shape: real service, real
    /// daemon ingest, no game server. Cleans its merged state files.
    #[test]
    fn bench_runs_small_shape_offline() {
        let config = BenchConfig { workers: 4, secs: 2, legs: 4, pace_ms: 200 };
        let report = run_bench(&config).expect("small bench runs offline");
        for index in 0..config.workers {
            let _ = std::fs::remove_file(
                crate::paths::users_dir().join(format!("bench-{index:03}.json")),
            );
        }
        assert_eq!(report.workers, 4);
        assert!(report.plans >= 4, "every worker planned, got {}", report.plans);
        assert!(report.pushes >= 4, "every worker pushed, got {}", report.pushes);
        assert_eq!(report.unknown, 0, "no lost tickets");
    }
}
