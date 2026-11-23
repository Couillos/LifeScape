#![forbid(unsafe_code)]
//! Nav ETA cache plus the live tick meter: one plan per departure zone
//! (cached across rounds), and the server tick length measured from the
//! snapshots themselves (no env, no dev/prod switch).

use std::collections::{HashMap, HashSet};

use lifescape_nav::plan::{NavOptions, Tile};


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

/// One blocking nav plan, daemon-side. Retries once on version skew
/// like every worker plan, then `None`: the caller leaves the deals
/// pending — never a local guess.
fn plan_eta_ticks(from: (i32, i32, i32), dest: Tile, version: &mut String) -> Option<u64> {
    let client =
        lifescape_nav::service::PlanClient::new(&crate::plan_service::socket_path(), "market-convene");
    let options = NavOptions {
        allow_paid: false,
        f2p_only: true,
        satisfied: HashSet::new(),
        excluded_edges: HashSet::new(), danger: Vec::new() };
    let from_tile = Tile { x: from.1, z: from.2, level: from.0 };
    for _ in 0..2 {
        match client.plan_once(from_tile, dest, &options, version, 8000) {
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
