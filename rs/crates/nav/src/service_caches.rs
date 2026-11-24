#![forbid(unsafe_code)]
//! The caches of the navigation service in one object (lot N6).
//!
//! The route cache (`route_cache`), the N8 best-served baselines and the
//! L2 edge sets with their pocket memo lived in `Service` as three bare
//! mutexes. Here each is reached through a method that holds its lock for
//! one operation and never two at once; none needs a socket, a pool
//! thread or a ticket, so each is tested on its own.
//!
//! The `walk_costs` batch cache that used to sit here is gone: its key
//! holds the exact start tile, and live traffic hit it 32 times in 3 010
//! batches (1 %). A batch costs a tenth of a millisecond after the dense
//! grid; building and comparing the key (the danger list included) cost
//! as much as the searches it saved.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::plan::{self, NavOptions, SharedEdges};
use crate::pocket::PocketMemo;
use crate::route_cache::RouteCache;
use crate::service::{PlanKey, Snapshot};
use crate::service_counters::Counters;

/// One shared edge set and its pocket memo, per data version.
pub(crate) type L2Slot = (Arc<SharedEdges>, Arc<PocketMemo>);

/// Edge sets kept: the current version and the one draining.
pub const L2_VERSIONS_CAP: usize = 2;

/// Best-served baseline cap: pairs, not bytes (keys are small). A full
/// map clears and re-baselines at zero.
const BEST_COSTS_CAP: usize = 4096;

/// N8 baseline core, pure: excess over the cheapest served cost for
/// the pair, and the new floor. First sighting and improvements read
/// 0 and re-baseline; a regression keeps the old floor so every bad
/// plan counts, not just the first.
pub(crate) fn best_detour(known: Option<u32>, cost: u32) -> (u32, u32) {
    match known {
        None => (0, cost),
        Some(floor) if cost <= floor => (0, cost),
        Some(floor) => (cost - floor, floor),
    }
}

pub(crate) struct Caches {
    /// Central route cache, one for every bot, bound to one version.
    pub(crate) routes: Mutex<RouteCache>,
    /// N8 best-served baselines: cheapest computed cost per PlanKey. A
    /// repeated pair must re-price identically (deterministic planner),
    /// so any excess is a regression, never geometry.
    best_costs: Mutex<HashMap<PlanKey, u32>>,
    /// L2: edge set + pocket memo per version, with the insertion order
    /// (the oldest leaves when `L2_VERSIONS_CAP` is exceeded).
    l2: Mutex<(HashMap<String, L2Slot>, Vec<String>)>,
}

impl Caches {
    pub(crate) fn new(version: &str) -> Caches {
        Caches {
            routes: Mutex::new(RouteCache::new(version)),
            best_costs: Mutex::new(HashMap::new()),
            l2: Mutex::new((HashMap::new(), Vec::new())),
        }
    }

    /// New world, new baselines: old floors never price new geometry.
    pub(crate) fn reset_baselines(&self) {
        self.best_costs.lock().unwrap().clear();
    }

    /// Record a computed plan cost for `key` and return its detour: the
    /// excess over the cheapest cost served so far for the same key.
    pub(crate) fn note_cost(&self, key: &PlanKey, cost: u32) -> u32 {
        let mut best = self.best_costs.lock().unwrap();
        if best.len() >= BEST_COSTS_CAP {
            best.clear();
        }
        let (detour, floor) = best_detour(best.get(key).copied(), cost);
        best.insert(key.clone(), floor);
        detour
    }

    /// Sealed-pocket verdicts memoized for `version`.
    pub(crate) fn pocket_verdicts(&self, version: &str) -> usize {
        self.l2.lock().unwrap().0.get(version).map(|(_, pockets)| pockets.len()).unwrap_or(0)
    }

    /// The memo's rows of `version` (saved by `nav_state`).
    pub(crate) fn pocket_rows(&self, version: &str) -> Vec<(u64, Option<Vec<u64>>)> {
        self.l2.lock().unwrap().0.get(version).map(|(_, memo)| memo.export()).unwrap_or_default()
    }

    /// LOT3 L2: one edge set per data version, shared by every search.
    /// A reload drains the old version while the new one computes once.
    pub(crate) fn shared_edges(&self, snapshot: &Arc<Snapshot>, counters: &Counters) -> Option<L2Slot> {
        if let Some(shared) = self.l2.lock().unwrap().0.get(&snapshot.version).cloned() {
            counters.l2_hits.fetch_add(1, Ordering::Relaxed);
            return Some(shared);
        }
        counters.l2_miss.fetch_add(1, Ordering::Relaxed);
        // The L2 is the permissive superset (requires/tolls/F2P lifted):
        // identical for every job, so a default options fits all.
        let mut always = || true;
        let edges = plan::prepare_shared(&snapshot.world, &snapshot.tables, &NavOptions::default(), &mut always)?;
        let shared: L2Slot = (Arc::new(edges), Arc::new(PocketMemo::default()));
        let mut l2 = self.l2.lock().unwrap();
        if let Some(raced) = l2.0.get(&snapshot.version) {
            // Another thread built it first: share its memo too.
            return Some(raced.clone());
        }
        l2.1.push(snapshot.version.clone());
        l2.0.insert(snapshot.version.clone(), shared.clone());
        while l2.0.len() > L2_VERSIONS_CAP {
            let oldest = l2.1.remove(0);
            l2.0.remove(&oldest);
        }
        Some(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Tile;

    fn key(dest_x: i32) -> PlanKey {
        let tile = |x: i32| Tile { x, z: 3200, level: 0 };
        PlanKey::of(tile(3200), tile(dest_x), &NavOptions::default(), "v")
    }

    #[test]
    fn best_detour_prices_regressions_only() {
        assert_eq!(best_detour(None, 13), (0, 13));
        assert_eq!(best_detour(Some(13), 13), (0, 13));
        assert_eq!(best_detour(Some(13), 9), (0, 9));
        assert_eq!(best_detour(Some(10), 13), (3, 10));
    }

    #[test]
    fn note_cost_baselines_then_counts_only_regressions() {
        let caches = Caches::new("v");
        assert_eq!(caches.note_cost(&key(3210), 10), 0, "first sighting baselines");
        assert_eq!(caches.note_cost(&key(3210), 10), 0, "same price reads zero");
        assert_eq!(caches.note_cost(&key(3210), 14), 4, "a worse plan for a known pair is a detour");
        assert_eq!(caches.note_cost(&key(3210), 14), 4, "the floor stays at the cheapest");
        assert_eq!(caches.note_cost(&key(3220), 99), 0, "another pair baselines on its own");
        caches.reset_baselines();
        assert_eq!(caches.note_cost(&key(3210), 14), 0, "a reload forgets the floors");
    }

    #[test]
    fn a_full_baseline_map_clears_and_starts_over() {
        let caches = Caches::new("v");
        for index in 0..BEST_COSTS_CAP as i32 {
            caches.note_cost(&key(3000 + index), 5);
        }
        assert_eq!(caches.best_costs.lock().unwrap().len(), BEST_COSTS_CAP);
        assert_eq!(caches.note_cost(&key(9000), 7), 0);
        assert_eq!(caches.best_costs.lock().unwrap().len(), 1, "cleared when full");
    }

    #[test]
    fn empty_caches_report_nothing() {
        let caches = Caches::new("v");
        assert_eq!(caches.pocket_verdicts("v"), 0);
        assert!(caches.pocket_rows("v").is_empty());
        assert_eq!(caches.routes.lock().unwrap().len(), 0);
    }
}
