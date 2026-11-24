#![forbid(unsafe_code)]
//! Nav ETA cache plus the live tick meter: one plan per departure zone
//! (cached across rounds), and the server tick length measured from the
//! snapshots themselves (no env, no dev/prod switch).

use std::collections::{HashMap, HashSet};

use lifescape_nav::plan::{NavOptions, Tile};
use lifescape_nav::service_ops::PlanRequest;


/// Zone granularity for the ETA cache (tiles >> 5).
const ZONE_SHIFT: u32 = 5;

/// Coarse zone of a position for the ETA cache.
pub fn zone_of(x: i32, z: i32) -> (i32, i32) {
    (x.div_euclid(1 << ZONE_SHIFT), z.div_euclid(1 << ZONE_SHIFT))
}

/// Coarse zone of a position is computed by the caller (`zone_of` lives
/// Live server tick length, measured from the snapshots themselves:
/// session ticks advance one per server tick, so wall-per-tick between
/// two sightings is the rate — no env, no dev/prod switch. Starts
/// prod-safe (600 ms), converges in a couple of rounds.
/// Last measured server tick length (ms), 0 before the first sample.
static LIVE_TICK_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Publishes the convene loop's measured tick length for the daemon.
pub fn publish_tick_ms(ms: u64) {
    LIVE_TICK_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// Server tick length as the daemon sees it live: the convene loop's
/// measurement, else the configured rate. The daemon runs without
/// `NODE_TICK_RATE`, so the configured fallback (600 ms) made every brain
/// wait in ticks last 10x too long against the 60 ms dev server (a market
/// fill wait of 6000 ticks = 1 hour instead of 6 minutes, 2026-11-13).
pub fn live_tick_ms() -> u64 {
    match LIVE_TICK_MS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => lifescape_sdk::waits::tick_ms(),
        measured => measured,
    }
}

pub struct TickMeter {
    last: Option<(f64, u64)>,
    ms: f64,
}

impl TickMeter {
    pub fn new() -> Self {
        TickMeter { last: None, ms: 600.0 }
    }

    /// Feed one (wall, tick) sighting; returns the current estimate.
    /// Out-of-order or duplicate ticks never move the estimate.
    pub fn observe(&mut self, wall: f64, tick: u64) -> u64 {
        if let Some((wall_before, tick_before)) = self.last {
            if tick > tick_before && wall > wall_before {
                let sample = (wall - wall_before) * 1000.0 / (tick - tick_before) as f64;
                if sample > 5.0 && sample < 5000.0 {
                    self.ms = self.ms * 0.7 + sample * 0.3;
                }
            }
        }
        self.last = Some((wall, tick));
        self.ms.round().max(1.0) as u64
    }
}

/// Nav ETA cache keyed by departure zone: one plan per zone, not per
/// deal and not per bot, so ten simultaneous deals cost a couple of
/// plans. Entries never expire within a daemon run (the world is
/// static); a reload-nav swap only changes costs marginally.
pub struct EtaCache {
    zones: HashMap<(i32, i32), u64>,
    version: String,
}

impl EtaCache {
    pub fn new() -> Self {
        EtaCache { zones: HashMap::new(), version: String::new() }
    }

    pub fn cached(&self, zone: (i32, i32)) -> Option<u64> {
        self.zones.get(&zone).cloned()
    }

    /// Zone ETA in ticks (`plan.cost` walks ~1 tile per tick): cache hit,
    /// else one blocking nav plan. Any failure is `None` and the deals
    /// wait — never a local guess.
    pub fn eta(&mut self, from: Tile, dest: Tile) -> Option<u64> {
        let zone = zone_of(from.x, from.z);
        if let Some(known) = self.zones.get(&zone) {
            return Some(*known);
        }
        let ticks = plan_eta_ticks((from.level, from.x, from.z), dest, &mut self.version)?;
        self.zones.insert(zone, ticks);
        Some(ticks)
    }
}

/// One blocking nav plan, daemon-side, through the service in this very
/// process (`Service::plan_in_process`: same admission, caches and
/// refusals as the socket `plan`, no frame). Retries once on version
/// skew like every worker plan, then `None`: the caller leaves the deals
/// pending — never a local guess. A service that is down is `None`.
fn plan_eta_ticks(from: (i32, i32, i32), dest: Tile, version: &mut String) -> Option<u64> {
    let service = crate::plan_service::service()?;
    eta_ticks_via(&service, from, dest, version)
}

fn eta_ticks_via(service: &lifescape_nav::service::Service, from: (i32, i32, i32), dest: Tile, version: &mut String) -> Option<u64> {
    let options = NavOptions {
        allow_paid: false,
        f2p_only: true,
        satisfied: HashSet::new(),
        excluded_edges: HashSet::new(), danger: Vec::new() };
    let from_tile = Tile { x: from.1, z: from.2, level: from.0 };
    for _ in 0..2 {
        let request = PlanRequest {
            account: Some("market-convene".to_string()),
            from: from_tile,
            dest,
            options: options.clone(),
            version: version.clone(),
        };
        match service.plan_in_process(request, 8000) {
            Ok((lifescape_nav::service::Poll::Ready(plan), served)) => {
                *version = served;
                return Some(plan.cost as u64);
            }
            Err(lifescape_nav::service_plan::PlanOnceError::Refused { served, .. }) if !served.is_empty() => {
                *version = served;
            }
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ETA through the service in this process equals the cost the
    /// socket `plan` gives the same pair (one daemon, one service, one
    /// answer), and a stale pinned version is adopted then retried once.
    #[test]
    fn eta_in_process_equals_the_socket_plan() {
        let snapshot = crate::plan_service::load_snapshot().expect("daemon snapshot loads");
        let sock = lifescape_nav::service::test_socket("eta-in-process");
        let service = lifescape_nav::service::Service::start(snapshot, &sock, 1).expect("service starts");
        let dest = Tile { x: 3214, z: 3429, level: 0 };
        let from = (0, 3222, 3218);
        let mut version = "stale".to_string();
        let ticks = eta_ticks_via(&service, from, dest, &mut version).expect("eta through the service");
        assert_eq!(version, service.current_version(), "the served version is adopted");
        let client = lifescape_nav::service::PlanClient::new(service.socket_path(), "market-convene");
        let options = NavOptions { allow_paid: false, f2p_only: true, satisfied: HashSet::new(), excluded_edges: HashSet::new(), danger: Vec::new() };
        let from_tile = Tile { x: from.1, z: from.2, level: from.0 };
        match client.plan_once(from_tile, dest, &options, &version, 8000) {
            Ok((lifescape_nav::service::Poll::Ready(plan), _)) => assert_eq!(plan.cost as u64, ticks),
            other => panic!("socket plan: {other:?}"),
        }
        let _ = std::fs::remove_file(&sock);
    }
}
