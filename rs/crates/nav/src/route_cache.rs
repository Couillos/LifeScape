#![forbid(unsafe_code)]
//! Central route cache of the pathfinding service (CPU 2026-11-14).
//!
//! One cache for every bot, owned by `Service`, bound to ONE snapshot
//! version (`reset` on reload: nothing computed on another world is ever
//! served; inserts from a draining pre-reload search are dropped).
//!
//! Positive entries hold the decoded plan plus its `PathInfo`
//! (`route_path`). They are NOT keyed by the full option set: a plan is
//! served to any request whose options it provably satisfies, exactly
//! (same cost as a fresh plan):
//! - `satisfied` tags: the planner's first search lifts requirements
//!   (`plan.rs` A), so a found plan does not depend on the tags, only on
//!   the tags its own requirement edges need → served when the request
//!   holds them;
//! - `allow_paid`: same value, or a paid-allowed plan that crosses no
//!   toll/fare served to a no-paid request (subgraph: the plan exists
//!   there and nothing cheaper can);
//! - banned edges: the entry's bans ⊆ the request's, and the plan
//!   crosses none of the request's bans (subgraph again);
//! - danger squares: the entry's squares ⊆ the request's, and the
//!   walked tiles avoid the extra squares (other paths only got dearer).
//!
//! Negative entries (NoRoute) stay keyed by the full `PlanKey` and live
//! `NEGATIVE_TTL`: they stop replan storms on a dead destination, then
//! recompute. LRU over both, bounded by count and bytes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lifescape_json::Value;

use crate::plan::{DangerZone, EdgeId, NavOptions, Plan, Tile};
use crate::route_path::{self, Arrival, PathInfo};
use crate::service::PlanKey;

/// Positive entries kept (one per computed plan, all bots together).
pub const ROUTE_CAP: usize = 8192;
/// Estimated bytes kept for positive entries (`PathInfo::bytes`).
pub const ROUTE_BYTES_CAP: usize = 64 * 1024 * 1024;
/// Negative verdicts (NoRoute) served this long, then recomputed.
pub const NEGATIVE_TTL: Duration = Duration::from_secs(60);
/// Negative entries kept (oldest dropped first).
pub const NEGATIVE_CAP: usize = 2048;
/// Spatial cell of the endpoint index (tiles >> 4: 16x16).
const CELL_SHIFT: i32 = 4;
/// Entries examined per reuse attempt (bounded work on a miss).
const CANDIDATES_CAP: usize = 48;
/// Newest entries read per cell list (hot cells hold many paths).
const PER_CELL: usize = 96;

/// One computed plan with what its reuse needs.
#[derive(Debug)]
pub(crate) struct PlanEntry {
    pub from: Tile,
    /// The requested destination (the plan may end adjusted beside it).
    pub dest: Tile,
    pub plan: Plan,
    pub f2p_only: bool,
    pub allow_paid: bool,
    pub excluded: HashSet<EdgeId>,
    pub danger: Vec<DangerZone>,
    pub info: PathInfo,
}

/// How a request was served from the cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReuseKind {
    /// Same endpoints, same options.
    Exact,
    /// Same endpoints, other options the plan provably satisfies.
    Dominated,
    /// Same start, destination on the cached path.
    Prefix,
    /// Same destination, start on the cached path.
    Suffix,
    /// Both endpoints on the cached path.
    Infix,
    /// Short local walks onto / off the cached path (epsilon rule).
    Splice,
}

/// Cache counters (monotonic) and occupancy.
#[derive(Clone, Copy, Default, Debug)]
pub struct RouteStats {
    pub exact: u64,
    pub dominated: u64,
    pub prefix: u64,
    pub suffix: u64,
    pub infix: u64,
    pub splice: u64,
    pub negative: u64,
    pub inserted: u64,
    pub evictions: u64,
    pub resets: u64,
    pub entries: usize,
    pub negatives: usize,
    pub bytes: usize,
}

impl RouteStats {
    pub fn count(&mut self, kind: ReuseKind) {
        match kind {
            ReuseKind::Exact => self.exact += 1,
            ReuseKind::Dominated => self.dominated += 1,
            ReuseKind::Prefix => self.prefix += 1,
            ReuseKind::Suffix => self.suffix += 1,
            ReuseKind::Infix => self.infix += 1,
            ReuseKind::Splice => self.splice += 1,
        }
    }
}

struct Slot {
    entry: Arc<PlanEntry>,
    /// 16x16 cells the path crosses (index maintenance).
    cells: Vec<CellKey>,
    stamp: u64,
    bytes: usize,
    inserted: Instant,
}

/// What the fast (locked, search-free) lookup found.
pub(crate) enum Fast {
    Plan(Arc<PlanEntry>),
    Negative(Value),
}

type PairKey = (bool, u64, u64);
type EndKey = (bool, u64);
type CellKey = (bool, i32, i32, i32);

/// The cache itself (the service wraps it in one mutex; no search ever
/// runs under that lock: splices take candidates out first).
pub(crate) struct RouteCache {
    version: String,
    next_id: u64,
    next_stamp: u64,
    plans: HashMap<u64, Slot>,
    order: BTreeMap<u64, u64>,
    by_pair: HashMap<PairKey, Vec<u64>>,
    /// Same start / same destination (exact tiles): the cheapest,
    /// likeliest sub-path candidates, read first.
    by_from: HashMap<EndKey, Vec<u64>>,
    by_dest: HashMap<EndKey, Vec<u64>>,
    /// Entries whose PATH crosses a 16x16 cell (start, middle or end):
    /// a request is a sub-path / splice candidate of an entry crossing
    /// the cells around both its endpoints.
    by_cells: HashMap<CellKey, Vec<u64>>,
    negatives: HashMap<PlanKey, (Value, Instant)>,
    bytes: usize,
    pub stats: RouteStats,
}

fn cell_of(tile: Tile) -> (i32, i32, i32) {
    (tile.x >> CELL_SHIFT, tile.z >> CELL_SHIFT, tile.level)
}

fn cell_key(f2p: bool, cell: (i32, i32, i32)) -> CellKey {
    (f2p, cell.0, cell.1, cell.2)
}

/// Distinct cells a path crosses, in path order.
fn path_cells(f2p: bool, path: &[Tile]) -> Vec<CellKey> {
    let mut cells: Vec<CellKey> = Vec::new();
    for tile in path {
        let key = cell_key(f2p, cell_of(*tile));
        if !cells.contains(&key) {
            cells.push(key);
        }
    }
    cells
}

/// Ids indexed in the 3x3 cells around `tile`, newest `PER_CELL` each.
fn ids_around(index: &HashMap<CellKey, Vec<u64>>, f2p: bool, tile: Tile) -> Vec<u64> {
    let (cell_x, cell_z, level) = cell_of(tile);
    let mut ids = Vec::new();
    for step_x in -1..=1 {
        for step_z in -1..=1 {
            if let Some(list) = index.get(&cell_key(f2p, (cell_x + step_x, cell_z + step_z, level))) {
                ids.extend(list.iter().rev().take(PER_CELL));
            }
        }
    }
    ids
}

impl RouteCache {
    pub fn new(version: &str) -> Self {
        RouteCache {
            version: version.to_string(),
            next_id: 0,
            next_stamp: 0,
            plans: HashMap::new(),
            order: BTreeMap::new(),
            by_pair: HashMap::new(),
            by_from: HashMap::new(),
            by_dest: HashMap::new(),
            by_cells: HashMap::new(),
            negatives: HashMap::new(),
            bytes: 0,
            stats: RouteStats::default(),
        }
    }

    /// New snapshot version: everything goes (reload / version bump).
    pub fn reset(&mut self, version: &str) {
        if self.version == version {
            return;
        }
        let stats = self.stats;
        *self = RouteCache::new(version);
        self.stats = stats;
        self.stats.resets += 1;
    }

    pub fn len(&self) -> usize {
        self.plans.len() + self.negatives.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn snapshot_stats(&self) -> RouteStats {
        let mut stats = self.stats;
        stats.entries = self.plans.len();
        stats.negatives = self.negatives.len();
        stats.bytes = self.bytes;
        stats
    }

    /// Age of the least recently used positive entry.
    pub fn oldest_age(&self) -> Option<Duration> {
        let (_, id) = self.order.iter().next()?;
        self.plans.get(id).map(|slot| slot.inserted.elapsed())
    }

    fn touch(&mut self, id: u64) {
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        if let Some(slot) = self.plans.get_mut(&id) {
            self.order.remove(&slot.stamp);
            slot.stamp = stamp;
            self.order.insert(stamp, id);
        }
    }

    /// Served from outside the lock (splice / sub-path): refresh + count.
    pub fn served(&mut self, id: u64, kind: ReuseKind) {
        self.touch(id);
        self.stats.count(kind);
    }

    /// Search-free lookup: live negative for the exact key, else a cached
    /// plan with the same endpoints whose options the request satisfies.
    pub fn fast(&mut self, version: &str, key: &PlanKey, from: Tile, dest: Tile, options: &NavOptions) -> Option<Fast> {
        if version != self.version {
            return None;
        }
        if let Some((body, expires)) = self.negatives.get(key) {
            if Instant::now() < *expires {
                let body = body.clone();
                self.stats.negative += 1;
                return Some(Fast::Negative(body));
            }
            self.negatives.remove(key);
        }
        let ids = self.by_pair.get(&(options.f2p_only, from.key(), dest.key()))?.clone();
        for id in ids {
            let Some(slot) = self.plans.get(&id) else { continue };
            let entry = Arc::clone(&slot.entry);
            let last = entry.info.last();
            if !compatible(&entry, options, 0, last) {
                continue;
            }
            let kind = if same_options(&entry, options) { ReuseKind::Exact } else { ReuseKind::Dominated };
            self.touch(id);
            self.stats.count(kind);
            return Some(Fast::Plan(entry));
        }
        None
    }

    /// Entries worth a sub-path or splice attempt: their path crosses
    /// the cells around the start AND around the destination (newest
    /// first). Only exactly priced entries (sub-paths need cumulative
    /// costs).
    pub fn candidates(&self, version: &str, from: Tile, dest: Tile, options: &NavOptions) -> Vec<(u64, Arc<PlanEntry>)> {
        if version != self.version {
            return Vec::new();
        }
        let f2p = options.f2p_only;
        let mut ids: Vec<u64> = Vec::new();
        for list in [self.by_dest.get(&(f2p, dest.key())), self.by_from.get(&(f2p, from.key()))].into_iter().flatten() {
            for id in list.iter().rev().take(CANDIDATES_CAP / 2) {
                if !ids.contains(id) {
                    ids.push(*id);
                }
            }
        }
        let near_from: HashSet<u64> = ids_around(&self.by_cells, f2p, from).into_iter().collect();
        for id in ids_around(&self.by_cells, f2p, dest) {
            if ids.len() >= CANDIDATES_CAP {
                break;
            }
            if near_from.contains(&id) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.into_iter()
            .filter_map(|id| self.plans.get(&id).map(|slot| (id, Arc::clone(&slot.entry))))
            .filter(|(_, entry)| entry.info.cumulative.is_some())
            .collect()
    }

    /// Stores one computed plan (a draining search of another version is
    /// dropped). A same-endpoint, same-option entry is replaced.
    pub fn insert_plan(&mut self, version: &str, entry: PlanEntry) {
        if version != self.version {
            return;
        }
        let pair = (entry.f2p_only, entry.from.key(), entry.dest.key());
        let replaced: Vec<u64> = self
            .by_pair
            .get(&pair)
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| self.plans.get(id).is_some_and(|slot| same_entry_options(&slot.entry, &entry)))
                    .collect()
            })
            .unwrap_or_default();
        for id in replaced {
            self.remove(id);
        }
        let id = self.next_id;
        self.next_id += 1;
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        let bytes = entry.info.bytes() + entry.plan.segments.len() * 48;
        let cells = path_cells(entry.f2p_only, &entry.info.path);
        self.by_pair.entry(pair).or_default().push(id);
        self.by_from.entry((entry.f2p_only, entry.from.key())).or_default().push(id);
        self.by_dest.entry((entry.f2p_only, entry.dest.key())).or_default().push(id);
        for cell in &cells {
            self.by_cells.entry(*cell).or_default().push(id);
        }
        self.plans.insert(id, Slot { entry: Arc::new(entry), cells, stamp, bytes, inserted: Instant::now() });
        self.order.insert(stamp, id);
        self.bytes += bytes;
        self.stats.inserted += 1;
        while self.plans.len() > ROUTE_CAP || self.bytes > ROUTE_BYTES_CAP {
            let Some((_, oldest)) = self.order.iter().next().map(|(stamp, id)| (*stamp, *id)) else { break };
            self.remove(oldest);
            self.stats.evictions += 1;
        }
    }

    /// Stores one NoRoute verdict under its full key for `ttl`.
    pub fn insert_negative(&mut self, version: &str, key: PlanKey, body: Value, ttl: Duration) {
        if version != self.version {
            return;
        }
        let now = Instant::now();
        if self.negatives.len() >= NEGATIVE_CAP {
            self.negatives.retain(|_, (_, expires)| *expires > now);
            if self.negatives.len() >= NEGATIVE_CAP {
                // Still full of live verdicts: drop the soonest to expire.
                if let Some(victim) = self.negatives.iter().min_by_key(|(_, (_, expires))| *expires).map(|(key, _)| key.clone()) {
                    self.negatives.remove(&victim);
                    self.stats.evictions += 1;
                }
            }
        }
        self.negatives.insert(key, (body, now + ttl));
    }

    fn remove(&mut self, id: u64) {
        let Some(slot) = self.plans.remove(&id) else { return };
        self.order.remove(&slot.stamp);
        self.bytes = self.bytes.saturating_sub(slot.bytes);
        let entry = &slot.entry;
        let f2p = entry.f2p_only;
        drop_id(&mut self.by_pair, &(f2p, entry.from.key(), entry.dest.key()), id);
        drop_id(&mut self.by_from, &(f2p, entry.from.key()), id);
        drop_id(&mut self.by_dest, &(f2p, entry.dest.key()), id);
        for cell in &slot.cells {
            drop_id(&mut self.by_cells, cell, id);
        }
    }

    /// RAM warn (§5.9): drop the older half of the positives and every
    /// negative; the newest half keeps serving.
    pub fn evict_half(&mut self) {
        let drop_count = self.plans.len() / 2;
        let victims: Vec<u64> = self.order.values().take(drop_count).copied().collect();
        for id in victims {
            self.remove(id);
            self.stats.evictions += 1;
        }
        self.stats.evictions += self.negatives.len() as u64;
        self.negatives.clear();
    }
}

fn drop_id<K: std::hash::Hash + Eq>(index: &mut HashMap<K, Vec<u64>>, key: &K, id: u64) {
    if let Some(list) = index.get_mut(key) {
        list.retain(|known| *known != id);
        if list.is_empty() {
            index.remove(key);
        }
    }
}

fn same_options(entry: &PlanEntry, options: &NavOptions) -> bool {
    entry.allow_paid == options.allow_paid && entry.excluded == options.excluded_edges && sorted(&entry.danger) == sorted(&options.danger)
}

fn same_entry_options(left: &PlanEntry, right: &PlanEntry) -> bool {
    left.allow_paid == right.allow_paid && left.excluded == right.excluded && sorted(&left.danger) == sorted(&right.danger)
}

fn sorted(zones: &[DangerZone]) -> Vec<DangerZone> {
    let mut zones = zones.to_vec();
    zones.sort();
    zones
}

/// True when the sub-path `(start, end]` of `entry`, optimal under the
/// entry's options, is a valid AND optimal plan under `options` (module
/// doc: requirement tags, paid edges, banned edges, danger squares).
pub(crate) fn compatible(entry: &PlanEntry, options: &NavOptions, start: usize, end: usize) -> bool {
    if entry.f2p_only != options.f2p_only {
        return false;
    }
    if !entry.allow_paid && options.allow_paid {
        return false;
    }
    if !entry.excluded.is_subset(&options.excluded_edges) {
        return false;
    }
    if !route_path::zones_covered(&entry.danger, &options.danger) {
        return false;
    }
    for edge in &entry.info.edges {
        if edge.at <= start || edge.at > end {
            continue;
        }
        if edge.paid && !options.allow_paid {
            return false;
        }
        if !edge.requires.iter().all(|tag| options.satisfied.contains(tag)) {
            return false;
        }
        if options.excluded_edges.contains(&edge.id) {
            return false;
        }
    }
    if !options.danger.is_empty() {
        for index in start + 1..=end.min(entry.info.last()) {
            let tile = entry.info.path[index];
            if entry.info.arrival[index] == Arrival::Walk
                && route_path::in_zones(&options.danger, tile)
                && !route_path::in_zones(&entry.danger, tile)
            {
                return false;
            }
        }
    }
    true
}
