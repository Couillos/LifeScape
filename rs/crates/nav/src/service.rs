//! Central pathfinding service, lot 3: shared world + global caches.
//!
//! The daemon loads one [`Snapshot`] (world + tables + version) and serves
//! plans over a Unix socket (JSON newline, same framing as the daemon
//! `send_worker` round-trips). Pool threads run the SAME `plan()` as the
//! sync path over a SHARED L2 edge set (one per data version); the global
//! L1 caches complete plans AND negatives under the full [`PlanKey`],
//! with exact-prefix truncation and in-flight joins on the same key.
//!
//! Locking (sequential by construction, never two held at once):
//! admission, L1/L2, in-flight, results, then the snapshot swap lock;
//! the pool never holds any lock during a Dijkstra (lookup/insert only).
//! No `unsafe`, std only (`mpsc::sync_channel` + threads).
//!
//! Bounds (lot 2, proposed): global queue 16, cap 2 in-flight tickets per
//! account, 2 plan threads, 256 stored results. Full/busy/stale submit ->
//! `Refused`, never a block: the caller surfaces typed `nav-down`,
//! logs it and counts it. A version skew (client vs server snapshot)
//! refuses stale too; the client resubmits once, never loops.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, object, str_value, Value};

use crate::plan::{
    self, Blocker, EdgeId, NavOptions, NoRoute, NoRouteReason, Plan, Segment, SharedEdges, Tile,
    TransitionRef,
};
use crate::service_queries;
use crate::static_world::StaticWorld;
use crate::tables::Tables;
use crate::{costs, walk_costs};

/// Global queue bound: submit past this is `Refused`, never queued.
/// T11 recalibration for 300 bots (E15 validates): 64 absorbs one
/// synchronized convocation wave without `nav-down`.
pub const QUEUE_CAP: usize = 64;
/// In-flight tickets (queued + computing) per account.
pub const ACCOUNT_CAP: usize = 2;
/// Plan threads sharing the snapshot (`~14 plans/s` on one thread at the
/// ~70 ms median; the second absorbs 1.5 s exhaustive NoRoutes).
/// T11 recalibration: 4 compute threads + the 16 `plan` waiters, so one
/// synchronized wave of 300 bots finds workers instead of `queue-full`.
pub const PLAN_THREADS: usize = 4;
/// Stored results bound: evicts consumable outcomes, never pendings.
/// T11: 512 holds the `plan`-op terminal burst of one wave (every waiter
/// poll marks its slot consumed, so eviction reaps them FIFO).
pub const RESULTS_CAP: usize = 512;
/// Submit is admission only (no plan runs inside): fast by construction.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(10);
/// One poll round-trip bound (map lookup, no compute inside).
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll pacing: IPC rhythm, not a game wait (the keepalive stays tick-driven).
pub const SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(2);
/// Client-side guard (mirrors the 30 s `UNLOAD` wall guard): cancel, then `nav-down`.
pub const SERVICE_WAIT_MAX: Duration = Duration::from_secs(30);
/// Acceptor read bound per connection (tiny newline frames).
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(5);

// -- version -----------------------------------------------------------------

/// FNV-1a over file bytes (missing file mixes its path + a marker, so a
/// deleted input still bumps the version). 16 hex chars, opaque.
pub fn data_version(files: &[PathBuf]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut mix = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    };
    for path in files {
        for byte in path.to_string_lossy().bytes() {
            mix(byte);
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                for byte in bytes {
                    mix(byte);
                }
            }
            Err(_) => {
                for byte in b"missing" {
                    mix(*byte);
                }
            }
        }
    }
    format!("{hash:016x}")
}

// -- snapshot ----------------------------------------------------------------

/// One world + one tables + their version, shared by every plan thread.
pub struct Snapshot {
    pub world: Arc<StaticWorld>,
    pub tables: Arc<Tables>,
    pub version: String,
}

impl Snapshot {
    pub fn new(world: Arc<StaticWorld>, tables: Arc<Tables>, version: String) -> Arc<Snapshot> {
        Arc::new(Snapshot {
            world,
            tables,
            version,
        })
    }
}

// -- key (posed lot 2, enforced lot 3 L1) ------------------------------------

/// Full cache key, set now so the protocol can never normalize an option
/// away: same key <=> same plan bit for bit (`plan.rs:242`). The lot 2
/// isolation test pins this (any single field flip changes the key).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlanKey {
    pub from: Tile,
    pub dest: Tile,
    pub allow_paid: bool,
    pub f2p_only: bool,
    pub satisfied: Vec<String>,
    pub excluded: Vec<String>,
    pub danger: Vec<crate::plan::DangerZone>,
    pub version: String,
}

impl PlanKey {
    pub fn of(from: Tile, dest: Tile, options: &NavOptions, version: &str) -> Self {
        let mut satisfied: Vec<String> = options.satisfied.iter().cloned().collect();
        satisfied.sort();
        let mut excluded: Vec<String> = options.excluded_edges.iter().map(edge_key).collect();
        excluded.sort();
        PlanKey {
            from,
            dest,
            allow_paid: options.allow_paid,
            f2p_only: options.f2p_only,
            satisfied,
            excluded,
            danger: {
                let mut danger = options.danger.clone();
                danger.sort();
                danger
            },
            version: version.to_string(),
        }
    }
}

fn edge_key(edge: &EdgeId) -> String {
    match edge {
        EdgeId::Door { loc, x, z, level } => format!("door:{loc}:{x},{z},{level}"),
        EdgeId::Ride(name) => format!("ride:{name}"),
        EdgeId::Custom(name) => format!("custom:{name}"),
    }
}

/// Batch cost cache key (N1): identical (from, dests, options, version)
/// batches repeat every ranking cycle (spot/foe pickers re-rank the same
/// window), so the second identical batch serves with no search. Small
/// and bounded (cleared when full — a cache, never truth); a version
/// bump makes old keys unreachable, like L1. Cache hits count in `hits`:
/// served from cache with no compute, plans and batches alike.
const COSTS_CACHE_CAP: usize = 256;

#[derive(Clone, PartialEq, Eq, Hash)]
struct CostsKey {
    from: Tile,
    dests: Vec<Tile>,
    allow_paid: bool,
    f2p_only: bool,
    satisfied: Vec<String>,
    excluded: Vec<String>,
    version: String,
}

impl CostsKey {
    fn of(from: Tile, dests: &[Tile], options: &NavOptions, version: &str) -> Self {
        CostsKey {
            from,
            dests: dests.to_vec(),
            allow_paid: options.allow_paid,
            f2p_only: options.f2p_only,
            satisfied: options_satisfied(options),
            excluded: options_excluded(options),
            version: version.to_string(),
        }
    }
}

// -- lot 3 cache -------------------------------------------------------------
//
// L1: complete plans AND negatives, keyed by the full PlanKey (options +
// data version, never normalized). A bumped version makes every old key
// unreachable: no flush, no stale serve, eviction does the rest.
// Truncation: a dest ON a cached plan serves the prefix with no search
// (optimal by substructure); anything else recomputes, never extends.
// L2: one shared edge set per data version (2 slots: current + draining).
//
// Revert (1 point): make `Lru::get`/`Lru::truncate` return None and skip
// the `insert` in `serve_plans` — the service is back to a lot2 wrapper
// (pool + shared L2) with zero behavior change. Version pinning stays:
// it is the staleness guard, not the cache.

/// Global plan cache bound: worst case 1024 x 9.6 Ko ~ 10 Mo + ~0.35 Mo
/// overhead (§5.9: 11 Mo line, never raised without rebouclage).
pub const L1_CAP: usize = 1024;
/// Entries above this are served once, never stored (§5.9; p99 alert 16 Ko).
pub const L1_MAX_ENTRY_BYTES: usize = 32 * 1024;
/// L2 versions kept: the live one plus the draining pre-reload one.
pub const L2_VERSIONS_CAP: usize = 2;

/// RAM §5.9: 1 Go hard cap, enforcement warn 80 % / shed 90 % / abort 95 %.
/// Mo entail MiB here (80 % of 1024 = 819.2 ≈ 820, 90 % = 921.6 ≈ 920).
pub const RAM_LIMIT_BYTES: u64 = 1024 * 1024 * 1024;
pub const RAM_WARN_BYTES: u64 = 820 * 1024 * 1024;
pub const RAM_SHED_BYTES: u64 = 920 * 1024 * 1024;

/// Admission verdict for one RSS reading: Pass serves, Warn serves after
/// logging + halving L1, Shed refuses (existing Refuse path, caller falls
/// back sync). Pure so the unit test injects thresholds with no memory
/// pressure; production calls it with the constants above.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RamGate {
    Pass,
    Warn,
    Shed,
}

pub fn ram_gate_for(rss_bytes: u64, warn_at: u64, shed_at: u64) -> RamGate {
    if rss_bytes >= shed_at {
        RamGate::Shed
    } else if rss_bytes >= warn_at {
        RamGate::Warn
    } else {
        RamGate::Pass
    }
}

/// Outcome counters, all atomic: the lot 3 hit-rate verdict reads these,
/// the lot 4 fairness verdict reads the refused_* ones (every non-stale
/// refusal surfaces as typed `nav-down` client-side, logged + counted).
#[derive(Clone, Copy, Default, Debug)]
pub struct CacheStats {
    pub hits: u64,
    pub trunc_hits: u64,
    /// Suffix serves (N2): the requester stood mid-plan, the tail served
    /// with no search, priced by substructure like a prefix.
    pub suffix_hits: u64,
    pub misses: u64,
    pub joins: u64,
    pub l2_hits: u64,
    pub l2_miss: u64,
    pub evictions: u64,
    pub refused_cap: u64,
    pub refused_full: u64,
    pub refused_stale: u64,
    pub unknown: u64,
    pub ram_warn: u64,
    pub ram_shed: u64,
    pub submits: u64,
    pub polls: u64,
    /// Multi-target cost batches served (N1), one search each.
    pub walk_costs: u64,
    /// Detour sums (N8): excess over the cheapest served cost for the
    /// same pair, accumulated over computed plans. `detour_sum /
    /// detour_plans` is the mean regression per plan (0 on healthy
    /// traffic); `detour_max` the worst single regression.
    pub detour_sum: u64,
    pub detour_max: u32,
    pub detour_plans: u64,
}

struct LruEntry {
    is_plan: bool,
    body: Value,
    bytes: usize,
    inserted: std::time::Instant,
}

struct Lru {
    map: HashMap<PlanKey, LruEntry>,
    order: VecDeque<PlanKey>,
    evictions: u64,
    stored_bytes: usize,
}

impl Lru {
    fn new() -> Self {
        Lru {
            map: HashMap::new(),
            order: VecDeque::new(),
            evictions: 0,
            stored_bytes: 0,
        }
    }

    fn get(&mut self, key: &PlanKey) -> Option<(bool, Value)> {
        let entry = self.map.get(key)?;
        let out = (entry.is_plan, entry.body.clone());
        // Refresh: most-recently-used goes back.
        if let Some(pos) = self.order.iter().position(|known| known == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
        Some(out)
    }

    /// Same from/options/version with dest on the cached tile path:
    /// exact prefix, no search (substructure-optimal, §5.3).
    fn truncate(
        &mut self,
        tables: &Tables,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
        version: &str,
    ) -> Option<Value> {
        // Cheap filter first (field compares, no decode); decode only the
        // few candidates sharing from/options/version.
        let mut candidates: Vec<(PlanKey, Value)> = Vec::new();
        for (key, entry) in self.map.iter() {
            if !entry.is_plan
                || key.from != from
                || key.dest == dest
                || key.version != version
                || key.allow_paid != options.allow_paid
                || key.f2p_only != options.f2p_only
                || key.satisfied != options_satisfied(options)
                || key.excluded != options_excluded(options)
            {
                continue;
            }
            candidates.push((key.clone(), entry.body.clone()));
        }
        for (key, body) in candidates {
            let Ok(cached) = decode_plan(&body) else {
                continue;
            };
            let Some(prefix) = truncate_plan(tables, from, &cached, dest) else {
                continue;
            };
            let value = encode_plan(&prefix);
            // The prefix serves through the same slot shape as a hit.
            self.get(&key);
            return Some(value);
        }
        None
    }

    /// Same dest/options/version with `from` ON a cached plan path:
    /// the tail from there, priced by substructure (cached cost minus
    /// the dropped prefix, edge by edge). Serves replans after a flight
    /// and mid-route joins with no search. Adjusted plans never serve:
    /// their last tile is not the destination.
    fn suffix(
        &mut self,
        tables: &Tables,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
        version: &str,
    ) -> Option<Value> {
        let mut candidates: Vec<(PlanKey, Value)> = Vec::new();
        for (key, entry) in self.map.iter() {
            if !entry.is_plan
                || key.dest != dest
                || key.from == from
                || key.version != version
                || key.allow_paid != options.allow_paid
                || key.f2p_only != options.f2p_only
                || key.satisfied != options_satisfied(options)
                || key.excluded != options_excluded(options)
            {
                continue;
            }
            candidates.push((key.clone(), entry.body.clone()));
        }
        for (key, body) in candidates {
            let Ok(cached) = decode_plan(&body) else {
                continue;
            };
            let Some(tail) = split_suffix(tables, key.from, &cached, from) else {
                continue;
            };
            let value = encode_plan(&tail);
            // The suffix serves through the same slot shape as a hit.
            self.get(&key);
            return Some(value);
        }
        None
    }

    fn insert(&mut self, key: PlanKey, is_plan: bool, body: Value) -> bool {
        let bytes = json::stringify(&body).len();
        if bytes > L1_MAX_ENTRY_BYTES {
            return false;
        }
        let now = std::time::Instant::now();
        if self.map.contains_key(&key) {
            if let Some(pos) = self.order.iter().position(|known| known == &key) {
                self.order.remove(pos);
            }
            if let Some(old) = self.map.get(&key) {
                self.stored_bytes = self.stored_bytes.saturating_sub(old.bytes);
            }
        }
        self.order.push_back(key.clone());
        self.stored_bytes += bytes;
        self.map.insert(key, LruEntry { is_plan, body, bytes, inserted: now });
        while self.map.len() > L1_CAP {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&oldest) {
                    self.stored_bytes = self.stored_bytes.saturating_sub(old.bytes);
                    self.evictions += 1;
                }
            } else {
                break;
            }
        }
        true
    }

    /// Age of the least-recently-used entry (front of the order): the L1
    /// max age the lot 4 metrics read. None when the cache is empty.
    fn oldest_age(&self) -> Option<std::time::Duration> {
        let now = std::time::Instant::now();
        self.order
            .front()
            .and_then(|key| self.map.get(key))
            .map(|entry| now.saturating_duration_since(entry.inserted))
    }

    /// Halve the cache oldest-first (§5.9 warn at 80 %): evictions counted,
    /// newest half keeps serving. Empty/small caches drop nothing.
    fn evict_half(&mut self) {
        let drop = self.map.len() / 2;
        for _ in 0..drop {
            if let Some(oldest) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&oldest) {
                    self.stored_bytes = self.stored_bytes.saturating_sub(old.bytes);
                    self.evictions += 1;
                }
            } else {
                break;
            }
        }
    }
}

/// Resident set size of this process in bytes (see `Service::rss_bytes`).
fn rss_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let resident_pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|field| field.parse().ok())
        .unwrap_or(0);
    resident_pages.saturating_mul(4096)
}

fn options_satisfied(options: &NavOptions) -> Vec<String> {    let mut satisfied: Vec<String> = options.satisfied.iter().cloned().collect();
    satisfied.sort();
    satisfied
}

fn options_excluded(options: &NavOptions) -> Vec<String> {
    let mut excluded: Vec<String> = options.excluded_edges.iter().map(edge_key).collect();
    excluded.sort();
    excluded
}

/// Walking-order tile path of a plan, `from` first: walk tiles in order,
/// door crossings, ride/custom landings. Stands are already the cursor.
fn plan_path(from: Tile, plan: &Plan) -> Vec<Tile> {
    let mut path = vec![from];
    for segment in &plan.segments {
        match segment {
            Segment::Walk { tiles } => path.extend(tiles.iter().copied()),
            Segment::OpenDoor { cross_to, .. } => path.push(*cross_to),
            Segment::Ride { expect, .. } | Segment::Custom { expect, .. } => path.push(*expect),
        }
    }
    path
}

/// Exact prefix of `cached` ending on `dest` (first occurrence: the
/// shortest prefix landing there). Costs are re-derived edge by edge with
/// the same formula as `assemble`, so the prefix prices exactly what a
/// fresh search would. `None`: dest is not on the path (recompute).
fn truncate_plan(tables: &Tables, from: Tile, cached: &Plan, dest: Tile) -> Option<Plan> {
    let path = plan_path(from, cached);
    let pos = path.iter().position(|tile| *tile == dest)?;
    if pos == 0 {
        return Some(Plan {
            segments: Vec::new(),
            cost: 0,
            adjusted_dest: None,
            dest_blocker: None,
        });
    }
    let mut remaining = pos;
    let mut segments = Vec::new();
    let mut cost = 0u32;
    for segment in &cached.segments {
        match segment {
            Segment::Walk { tiles } => {
                if remaining > tiles.len() {
                    cost += tiles.len() as u32;
                    remaining -= tiles.len();
                    segments.push(segment.clone());
                } else {
                    cost += remaining as u32;
                    segments.push(Segment::Walk {
                        tiles: tiles[..remaining].to_vec(),
                    });
                    remaining = 0;
                    break;
                }
            }
            Segment::OpenDoor { door, stand, cross_to } => {
                // One segment per opening, one or two Dijkstra edges:
                // stepping on then straight off the same door merges two
                // edges into one segment (assemble), while stopping on the
                // tile or starting from it rides one. Count per edge, so
                // the prefix prices exactly what a fresh search charges.
                let door_tile = Tile {
                    x: door.x,
                    z: door.z,
                    level: door.level,
                };
                let edges = u32::from(*stand != door_tile) + u32::from(*cross_to != door_tile);
                cost += edges * plan::DOOR_COST;
                remaining -= 1;
                segments.push(segment.clone());
                if remaining == 0 {
                    break;
                }
            }
            Segment::Ride { transition, .. } => {
                cost += plan::TRANSITION_COST
                    + if tables
                        .verdict(transition.loc, transition.from)
                        .is_some_and(crate::tables::Verdict::verified)
                    {
                        0
                    } else {
                        plan::UNVERIFIED_PENALTY
                    };
                remaining -= 1;
                segments.push(segment.clone());
                if remaining == 0 {
                    break;
                }
            }
            Segment::Custom { .. } => {
                cost += plan::TRANSITION_COST + plan::UNVERIFIED_PENALTY;
                remaining -= 1;
                segments.push(segment.clone());
                if remaining == 0 {
                    break;
                }
            }
        }
    }
    if remaining != 0 {
        return None;
    }
    Some(Plan {
        segments,
        cost,
        adjusted_dest: None,
        dest_blocker: None,
    })
}

/// Tail of `cached` from `from` (which must lie strictly inside its tile
/// path), priced by substructure: cached cost minus the dropped prefix,
/// the prefix priced edge by edge with the same formula as `assemble`.
/// `None`: `from` is not on the path, or the plan is adjusted (its last
/// tile is not the destination, so no tail ends there).
fn split_suffix(tables: &Tables, cached_from: Tile, cached: &Plan, from: Tile) -> Option<Plan> {
    if cached.adjusted_dest.is_some() {
        return None;
    }
    let path = plan_path(cached_from, cached);
    let pos = path.iter().position(|tile| *tile == from)?;
    if pos == 0 {
        return None;
    }
    let mut remaining = pos;
    let mut prefix_cost = 0u32;
    let mut suffix = Vec::new();
    for segment in &cached.segments {
        if remaining == 0 {
            suffix.push(segment.clone());
            continue;
        }
        match segment {
            Segment::Walk { tiles } => {
                if remaining >= tiles.len() {
                    prefix_cost += tiles.len() as u32;
                    remaining -= tiles.len();
                } else {
                    // Split inside this walk: drop `remaining`, keep the rest.
                    prefix_cost += remaining as u32;
                    let keep = tiles[remaining..].to_vec();
                    if !keep.is_empty() {
                        suffix.push(Segment::Walk { tiles: keep });
                    }
                    remaining = 0;
                }
            }
            Segment::OpenDoor { door, stand, cross_to } => {
                // One path tile per opening; one or two Dijkstra edges
                // (same count as `truncate_plan`).
                let door_tile = Tile {
                    x: door.x,
                    z: door.z,
                    level: door.level,
                };
                let edges = u32::from(*stand != door_tile) + u32::from(*cross_to != door_tile);
                prefix_cost += edges * plan::DOOR_COST;
                remaining -= 1;
            }
            Segment::Ride { transition, .. } => {
                prefix_cost += plan::TRANSITION_COST
                    + if tables
                        .verdict(transition.loc, transition.from)
                        .is_some_and(crate::tables::Verdict::verified)
                    {
                        0
                    } else {
                        plan::UNVERIFIED_PENALTY
                    };
                remaining -= 1;
            }
            Segment::Custom { .. } => {
                prefix_cost += plan::TRANSITION_COST + plan::UNVERIFIED_PENALTY;
                remaining -= 1;
            }
        }
    }
    if remaining != 0 {
        return None;
    }
    Some(Plan {
        cost: cached.cost.saturating_sub(prefix_cost),
        segments: suffix,
        adjusted_dest: None,
        dest_blocker: None,
    })
}

// -- JSON codec (options serialized integrally, never normalized) -------------

pub(crate) fn tile_value(tile: Tile) -> Value {    object(vec![
        ("x", Value::Int(i64::from(tile.x))),
        ("z", Value::Int(i64::from(tile.z))),
        ("level", Value::Int(i64::from(tile.level))),
    ])
}

fn parse_tile(value: &Value) -> Result<Tile, String> {
    Ok(Tile {
        x: value.get("x").as_i64().ok_or("tile.x")? as i32,
        z: value.get("z").as_i64().ok_or("tile.z")? as i32,
        level: value.get("level").as_i64().ok_or("tile.level")? as i32,
    })
}

fn edge_value(edge: &EdgeId) -> Value {
    match edge {
        EdgeId::Door { loc, x, z, level } => object(vec![
            ("kind", str_value("door")),
            ("loc", Value::Int(i64::from(*loc))),
            ("x", Value::Int(i64::from(*x))),
            ("z", Value::Int(i64::from(*z))),
            ("level", Value::Int(i64::from(*level))),
        ]),
        EdgeId::Ride(name) => object(vec![("kind", str_value("ride")), ("name", str_value(name))]),
        EdgeId::Custom(name) => object(vec![
            ("kind", str_value("custom")),
            ("name", str_value(name)),
        ]),
    }
}

fn parse_edge(value: &Value) -> Result<EdgeId, String> {
    match value.get("kind").as_str().unwrap_or("") {
        "door" => Ok(EdgeId::Door {
            loc: value.get("loc").as_i64().ok_or("door.loc")? as u16,
            x: value.get("x").as_i64().ok_or("door.x")? as i32,
            z: value.get("z").as_i64().ok_or("door.z")? as i32,
            level: value.get("level").as_i64().ok_or("door.level")? as i32,
        }),
        "ride" => Ok(EdgeId::Ride(
            value.get("name").as_str().ok_or("ride.name")?.to_string(),
        )),
        "custom" => Ok(EdgeId::Custom(
            value.get("name").as_str().ok_or("custom.name")?.to_string(),
        )),
        kind => Err(format!("edge kind {kind:?}")),
    }
}

pub fn encode_options(options: &NavOptions) -> Value {
    let mut satisfied: Vec<String> = options.satisfied.iter().cloned().collect();
    satisfied.sort();
    let mut excluded: Vec<String> = options.excluded_edges.iter().map(edge_key).collect();
    excluded.sort();
    object(vec![
        ("allow_paid", Value::Bool(options.allow_paid)),
        ("f2p_only", Value::Bool(options.f2p_only)),
        (
            "satisfied",
            Value::Arr(satisfied.into_iter().map(Value::Str).collect()),
        ),
        (
            "excluded",
            Value::Arr(excluded.into_iter().map(Value::Str).collect()),
        ),
        (
            "excluded_edges",
            Value::Arr(options.excluded_edges.iter().map(edge_value).collect()),
        ),
        (
            "danger",
            Value::Arr(
                options
                    .danger
                    .iter()
                    .map(|zone| Value::Arr(vec![Value::Int(i64::from(zone.x)), Value::Int(i64::from(zone.z)), Value::Int(i64::from(zone.level)), Value::Int(i64::from(zone.radius))]))
                    .collect(),
            ),
        ),
    ])
}

pub fn decode_options(value: &Value) -> Result<NavOptions, String> {
    let mut options = NavOptions {
        allow_paid: value.get("allow_paid").as_bool().ok_or("allow_paid")?,
        f2p_only: value.get("f2p_only").as_bool().ok_or("f2p_only")?,
        satisfied: std::collections::HashSet::new(),
        excluded_edges: std::collections::HashSet::new(), danger: Vec::new() };
    for tag in value.get("satisfied").as_arr().ok_or("satisfied")? {
        options
            .satisfied
            .insert(tag.as_str().ok_or("satisfied tag")?.to_string());
    }
    // Canonical read: structured edges; the flat `excluded` strings are
    // a log form only (same content, key order).
    for edge in value
        .get("excluded_edges")
        .as_arr()
        .ok_or("excluded_edges")?
    {
        options.excluded_edges.insert(parse_edge(edge)?);
    }
    // Optional (older clients send none): [x, z, level, radius] squares.
    for zone in value.get("danger").as_arr().map(|rows| rows.to_vec()).unwrap_or_default() {
        let part = |slot: usize| zone.as_arr().and_then(|cells| cells.get(slot)).and_then(|cell| cell.as_i64()).map(|cell| cell as i32).ok_or("danger zone");
        options.danger.push(crate::plan::DangerZone { x: part(0)?, z: part(1)?, level: part(2)?, radius: part(3)? });
    }
    Ok(options)
}

fn ride_value(ride: &TransitionRef) -> Value {
    object(vec![
        ("name", str_value(&ride.name)),
        ("loc", Value::Int(i64::from(ride.loc))),
        ("climb_loc", Value::Int(i64::from(ride.climb_loc))),
        ("op", Value::Int(i64::from(ride.op))),
        ("from", tile_value(ride.from)),
        ("to", tile_value(ride.to)),
        (
            "footprint",
            Value::Arr(
                ride.footprint
                    .iter()
                    .map(|(x, z)| {
                        Value::Arr(vec![Value::Int(i64::from(*x)), Value::Int(i64::from(*z))])
                    })
                    .collect(),
            ),
        ),
        ("needs_choice", Value::Bool(ride.needs_choice)),
    ])
}

fn parse_ride(value: &Value) -> Result<TransitionRef, String> {
    let mut footprint = Vec::new();
    for pair in value.get("footprint").as_arr().ok_or("footprint")? {
        let pair = pair.as_arr().ok_or("footprint pair")?;
        footprint.push((
            pair.first().and_then(Value::as_i64).ok_or("footprint x")? as i32,
            pair.get(1).and_then(Value::as_i64).ok_or("footprint z")? as i32,
        ));
    }
    Ok(TransitionRef {
        name: value.get("name").as_str().ok_or("ride.name")?.to_string(),
        loc: value.get("loc").as_i64().ok_or("ride.loc")? as u16,
        climb_loc: value.get("climb_loc").as_i64().ok_or("ride.climb_loc")? as u16,
        op: value.get("op").as_i64().ok_or("ride.op")? as u8,
        from: parse_tile(value.get("from"))?,
        to: parse_tile(value.get("to"))?,
        footprint,
        // Old plans predate the field: no recorded pick suffix by default.
        needs_choice: value.get("needs_choice").as_bool().unwrap_or(false),
    })
}

fn segment_value(segment: &Segment) -> Value {
    match segment {
        Segment::Walk { tiles } => object(vec![
            ("kind", str_value("walk")),
            (
                "tiles",
                Value::Arr(tiles.iter().map(|tile| tile_value(*tile)).collect()),
            ),
        ]),
        Segment::OpenDoor {
            door,
            stand,
            cross_to,
        } => object(vec![
            ("kind", str_value("door")),
            (
                "door",
                object(vec![
                    ("loc", Value::Int(i64::from(door.loc))),
                    ("x", Value::Int(i64::from(door.x))),
                    ("z", Value::Int(i64::from(door.z))),
                    ("level", Value::Int(i64::from(door.level))),
                    // Absent op encodes null (never 0: 1-based op index).
                    (
                        "open_op",
                        door.open_op.map_or(Value::Null, |op| Value::Int(i64::from(op))),
                    ),
                    ("toll", Value::Bool(door.toll)),
                ]),
            ),
            ("stand", tile_value(*stand)),
            ("cross_to", tile_value(*cross_to)),
        ]),
        Segment::Ride {
            transition,
            stand,
            pre_open,
            expect,
        } => {
            let mut fields = vec![
                ("kind", str_value("ride")),
                ("ride", ride_value(transition)),
                ("stand", tile_value(*stand)),
                ("expect", tile_value(*expect)),
            ];
            if let Some(open) = pre_open {
                fields.push((
                    "pre_open",
                    object(vec![
                        ("loc", Value::Int(i64::from(open.loc))),
                        ("op", Value::Int(i64::from(open.op))),
                        ("open_to", Value::Int(i64::from(open.open_to))),
                    ]),
                ));
            }
            object(fields)
        }
        Segment::Custom {
            edge,
            stand,
            expect,
        } => {
            let mut fields = vec![
                ("kind", str_value("custom")),
                ("name", str_value(&edge.name)),
                ("stand", tile_value(*stand)),
                ("expect", tile_value(*expect)),
            ];
            fields.extend(edge.fields());
            object(fields)
        }
    }
}

fn parse_segment(value: &Value) -> Result<Segment, String> {
    match value.get("kind").as_str().unwrap_or("") {
        "walk" => {
            let mut tiles = Vec::new();
            for tile in value.get("tiles").as_arr().ok_or("walk.tiles")? {
                tiles.push(parse_tile(tile)?);
            }
            Ok(Segment::Walk { tiles })
        }
        "door" => Ok(Segment::OpenDoor {
            door: crate::plan::DoorRef {
                loc: value.get("door").get("loc").as_i64().ok_or("door.loc")? as u16,
                x: value.get("door").get("x").as_i64().ok_or("door.x")? as i32,
                z: value.get("door").get("z").as_i64().ok_or("door.z")? as i32,
                level: value
                    .get("door")
                    .get("level")
                    .as_i64()
                    .ok_or("door.level")? as i32,
                // Old plans predate the fields: shut door, never auto-paid.
                open_op: value.get("door").get("open_op").as_i64().map(|op| op as u8),
                toll: value.get("door").get("toll").as_bool().unwrap_or(false),
            },
            stand: parse_tile(value.get("stand"))?,
            cross_to: parse_tile(value.get("cross_to"))?,
        }),
        "ride" => {
            let pre_open = if value.get("pre_open").is_null() {
                None
            } else {
                Some(crate::plan::OpenStep {
                    loc: value
                        .get("pre_open")
                        .get("loc")
                        .as_i64()
                        .ok_or("pre_open.loc")? as u16,
                    op: value
                        .get("pre_open")
                        .get("op")
                        .as_i64()
                        .ok_or("pre_open.op")? as u8,
                    open_to: value
                        .get("pre_open")
                        .get("open_to")
                        .as_i64()
                        .ok_or("pre_open.open_to")? as u16,
                })
            };
            Ok(Segment::Ride {
                transition: parse_ride(value.get("ride"))?,
                stand: parse_tile(value.get("stand"))?,
                pre_open,
                expect: parse_tile(value.get("expect"))?,
            })
        }
        "custom" => Ok(Segment::Custom {
            edge: crate::plan::CustomEdgeRef::parse(value)?,
            stand: parse_tile(value.get("stand"))?,
            expect: parse_tile(value.get("expect"))?,
        }),
        kind => Err(format!("segment kind {kind:?}")),
    }
}

pub fn encode_plan(plan: &Plan) -> Value {
    let mut fields = vec![
        ("cost", Value::Int(i64::from(plan.cost))),
        (
            "segments",
            Value::Arr(plan.segments.iter().map(segment_value).collect()),
        ),
    ];
    if let Some(adjusted) = plan.adjusted_dest {
        fields.push(("adjusted_dest", tile_value(adjusted)));
    }
    // Always present (null on exact plans): old readers ignore it.
    fields.push((
        "dest_blocker",
        plan.dest_blocker.as_ref().map_or(Value::Null, blocker_value),
    ));
    object(fields)
}

fn blocker_value(blocker: &Blocker) -> Value {
    let mut fields = vec![
        ("loc", Value::Int(i64::from(blocker.loc))),
        ("name", str_value(&blocker.name)),
        ("x", Value::Int(i64::from(blocker.x))),
        ("z", Value::Int(i64::from(blocker.z))),
        ("level", Value::Int(i64::from(blocker.level))),
        ("door", Value::Bool(blocker.is_door)),
    ];
    fields.push((
        "open_op",
        blocker.open_op.map_or(Value::Null, |op| Value::Int(i64::from(op))),
    ));
    fields.push((
        "stand",
        blocker.stand.map_or(Value::Null, tile_value),
    ));
    object(fields)
}

fn parse_blocker(value: &Value) -> Result<Blocker, String> {
    Ok(Blocker {
        loc: value.get("loc").as_i64().ok_or("blocker.loc")? as u16,
        name: value.get("name").as_str().ok_or("blocker.name")?.to_string(),
        x: value.get("x").as_i64().ok_or("blocker.x")? as i32,
        z: value.get("z").as_i64().ok_or("blocker.z")? as i32,
        level: value.get("level").as_i64().ok_or("blocker.level")? as i32,
        open_op: value.get("open_op").as_i64().map(|op| op as u8),
        is_door: value.get("door").as_bool().unwrap_or(false),
        stand: if value.get("stand").is_null() {
            None
        } else {
            Some(parse_tile(value.get("stand"))?)
        },
    })
}

pub fn decode_plan(value: &Value) -> Result<Plan, String> {
    let mut segments = Vec::new();
    for segment in value.get("segments").as_arr().ok_or("segments")? {
        segments.push(parse_segment(segment)?);
    }
    Ok(Plan {
        segments,
        cost: value.get("cost").as_i64().ok_or("cost")? as u32,
        adjusted_dest: if value.get("adjusted_dest").is_null() {
            None
        } else {
            Some(parse_tile(value.get("adjusted_dest"))?)
        },
        // Old plans predate the field: exact destination, no blocker.
        dest_blocker: if value.get("dest_blocker").is_null() {
            None
        } else {
            Some(parse_blocker(value.get("dest_blocker"))?)
        },
    })
}

pub fn encode_noroute(no: &NoRoute) -> Value {
    object(vec![
        ("reason", str_value(no.reason.as_str())),
        ("closest", tile_value(no.closest_reachable)),
        (
            "blocked",
            Value::Arr(
                no.blocked_by_requirements
                    .iter()
                    .map(|(edge, missing)| {
                        object(vec![
                            ("edge", edge_value(edge)),
                            (
                                "missing",
                                Value::Arr(missing.iter().map(|tag| str_value(tag)).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "disabled",
            Value::Arr(
                no.disabled_transitions_nearby
                    .iter()
                    .map(ride_value)
                    .collect(),
            ),
        ),
        // Always present (empty on old-style diagnoses): what seals the
        // destination — loc names, door ops and click sides included.
        (
            "blockers",
            Value::Arr(no.blockers.iter().map(blocker_value).collect()),
        ),
    ])
}

pub fn decode_noroute(value: &Value) -> Result<NoRoute, String> {
    let mut blocked = Vec::new();
    for entry in value.get("blocked").as_arr().ok_or("blocked")? {
        let mut missing = Vec::new();
        for tag in entry.get("missing").as_arr().ok_or("missing")? {
            missing.push(tag.as_str().ok_or("missing tag")?.to_string());
        }
        blocked.push((parse_edge(entry.get("edge"))?, missing));
    }
    let mut disabled = Vec::new();
    for ride in value.get("disabled").as_arr().ok_or("disabled")? {
        disabled.push(parse_ride(ride)?);
    }
    // Old dossiers predate the field: no named seal, same verdict.
    let mut blockers = Vec::new();
    if let Some(rows) = value.get("blockers").as_arr() {
        for row in rows {
            blockers.push(parse_blocker(row)?);
        }
    }
    Ok(NoRoute {
        closest_reachable: parse_tile(value.get("closest"))?,
        blocked_by_requirements: blocked,
        disabled_transitions_nearby: disabled,
        reason: NoRouteReason::parse(value.get("reason").as_str().ok_or("reason")?)?,
        blockers,
    })
}

// -- server ------------------------------------------------------------------

struct Job {
    id: u64,
    account: String,
    key: PlanKey,
    from: Tile,
    dest: Tile,
    options: NavOptions,
    cancel: Arc<AtomicBool>,
}

enum Slot {
    Pending { cancel: Arc<AtomicBool> },
    Ready { plan: Value, polled: bool },
    Failed { error: Value, polled: bool },
}

/// Plan server: snapshot shared by the pool, bounded queue, IPC replies.
/// LOT3: global L1 (plans + negatives, keyed by the full PlanKey), global
/// L2 (one edge set per data version), in-flight joins on the full key.
/// Field order mirrors the lock discipline (admission, cache, in-flight,
/// results, snapshot); the pool never holds any lock during a Dijkstra.
pub struct Service {
    admission: Mutex<HashMap<String, usize>>,
    l1: Mutex<Lru>,
    costs_cache: Mutex<HashMap<CostsKey, Vec<Option<u32>>>>,
    /// N8 best-served baselines: cheapest computed cost per PlanKey. A
    /// repeated pair must re-price identically (deterministic planner),
    /// so any excess is a regression, never geometry.
    best_costs: Mutex<HashMap<PlanKey, u32>>,
    l2: Mutex<(HashMap<String, Arc<SharedEdges>>, Vec<String>)>,
    inflight: Mutex<HashMap<PlanKey, u64>>,
    tickets: Mutex<HashMap<u64, (PlanKey, String)>>,
    waiters: Mutex<HashMap<u64, usize>>,
    results: Mutex<HashMap<u64, Slot>>,
    snapshot: Mutex<Arc<Snapshot>>,
    queue_tx: SyncSender<Job>,
    queue_rx: Mutex<Receiver<Job>>,
    queued: AtomicUsize,
    next_id: AtomicU64,
    hits: AtomicU64,
    trunc_hits: AtomicU64,
    suffix_hits: AtomicU64,
    misses: AtomicU64,
    joins: AtomicU64,
    l2_hits: AtomicU64,
    l2_miss: AtomicU64,
    refused_cap: AtomicU64,
    refused_full: AtomicU64,
    refused_stale: AtomicU64,
    unknown_polls: AtomicU64,
    ram_warn: AtomicU64,
    ram_shed: AtomicU64,
    door_defs_served: AtomicU64,
    tiles_flags_served: AtomicU64,
    static_locs_served: AtomicU64,
    f2p_served: AtomicU64,
    toll_fence_served: AtomicU64,
    wants_served: AtomicU64,
    diff_served: AtomicU64,
    debug_served: AtomicU64,
    transitions_list_served: AtomicU64,
    probe_targets_served: AtomicU64,
    doors_near_served: AtomicU64,
    submits_served: AtomicU64,
    polls_served: AtomicU64,
    walk_costs_served: AtomicU64,
    detour_sum: AtomicU64,
    detour_max: AtomicU32,
    detour_plans: AtomicU64,
    sock_path: PathBuf,
    /// E3 waiter pool for the `plan` op (single-round-trip plans): bound
    /// filled at `start`, read by `plan_handoff`.
    wait_tx: Mutex<Option<mpsc::SyncSender<crate::service_plan::WaitJob>>>,
}

impl Service {
    /// Bind the socket and spawn the acceptor + `threads` plan workers.
    /// A live socket refuses a second server (stale file is reclaimed).
    pub fn start(
        snapshot: Arc<Snapshot>,
        sock: &Path,
        threads: usize,
    ) -> Result<Arc<Service>, String> {
        if sock.exists() {
            if UnixStream::connect(sock).is_ok() {
                return Err(format!("pathfinding already serving on {}", sock.display()));
            }
            let _ = std::fs::remove_file(sock);
        }
        let listener = UnixListener::bind(sock)
            .map_err(|error| format!("bind {}: {error}", sock.display()))?;
        let (queue_tx, queue_rx) = mpsc::sync_channel(QUEUE_CAP);
        let service = Arc::new(Service {
            admission: Mutex::new(HashMap::new()),
            l1: Mutex::new(Lru::new()),
            costs_cache: Mutex::new(HashMap::new()),
            best_costs: Mutex::new(HashMap::new()),
            l2: Mutex::new((HashMap::new(), Vec::new())),
            inflight: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            results: Mutex::new(HashMap::new()),
            snapshot: Mutex::new(snapshot),
            queue_tx,
            queue_rx: Mutex::new(queue_rx),
            queued: AtomicUsize::new(0),
            next_id: AtomicU64::new(1),
            hits: AtomicU64::new(0),
            trunc_hits: AtomicU64::new(0),
            suffix_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            joins: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            l2_miss: AtomicU64::new(0),
            refused_cap: AtomicU64::new(0),
            refused_full: AtomicU64::new(0),
            refused_stale: AtomicU64::new(0),
            unknown_polls: AtomicU64::new(0),
            ram_warn: AtomicU64::new(0),
            ram_shed: AtomicU64::new(0),
            door_defs_served: AtomicU64::new(0),
            tiles_flags_served: AtomicU64::new(0),
            static_locs_served: AtomicU64::new(0),
            f2p_served: AtomicU64::new(0),
            toll_fence_served: AtomicU64::new(0),
            wants_served: AtomicU64::new(0),
            diff_served: AtomicU64::new(0),
            debug_served: AtomicU64::new(0),
            transitions_list_served: AtomicU64::new(0),
            probe_targets_served: AtomicU64::new(0),
            doors_near_served: AtomicU64::new(0),
            submits_served: AtomicU64::new(0),
            polls_served: AtomicU64::new(0),
            walk_costs_served: AtomicU64::new(0),
            detour_sum: AtomicU64::new(0),
            detour_max: AtomicU32::new(0),
            detour_plans: AtomicU64::new(0),
            sock_path: sock.to_path_buf(),
            wait_tx: Mutex::new(None),
        });
        for index in 0..threads.max(1) {
            let worker = Arc::clone(&service);
            std::thread::Builder::new()
                .name(format!("plan-pool-{index}"))
                .spawn(move || worker.serve_plans())
                .expect("plan pool spawns");
        }
        let acceptor = Arc::clone(&service);
        std::thread::Builder::new()
            .name("plan-accept".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => acceptor.serve_one(stream),
                        Err(_) => break,
                    }
                }
            })
            .expect("plan acceptor spawns");
        // E3 waiter pool (single-round-trip `plan` ops): bounded threads
        // + queue, filled here so `plan_handoff` never blocks the acceptor.
        *service.wait_tx.lock().unwrap() = Some(crate::service_plan::start_wait_pool(service.clone(), threads));
        Ok(service)
    }

    pub fn socket_path(&self) -> &Path {
        &self.sock_path
    }

    /// Current snapshot version (operator gauge + skew logs).
    pub fn current_version(&self) -> String {
        self.version()
    }

    /// Live snapshot for read-only daemon consumers (handover ring).
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Lot 3 counters: hit-rate verdict reads these, not the logs.
    /// Lot 4 adds the refused_* ones: the fairness verdict reads those.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            trunc_hits: self.trunc_hits.load(Ordering::Relaxed),
            suffix_hits: self.suffix_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            joins: self.joins.load(Ordering::Relaxed),
            l2_hits: self.l2_hits.load(Ordering::Relaxed),
            l2_miss: self.l2_miss.load(Ordering::Relaxed),
            evictions: self.l1.lock().unwrap().evictions,
            refused_cap: self.refused_cap.load(Ordering::Relaxed),
            refused_full: self.refused_full.load(Ordering::Relaxed),
            refused_stale: self.refused_stale.load(Ordering::Relaxed),
            unknown: self.unknown_polls.load(Ordering::Relaxed),
            ram_warn: self.ram_warn.load(Ordering::Relaxed),
            ram_shed: self.ram_shed.load(Ordering::Relaxed),
            submits: self.submits_served.load(Ordering::Relaxed),
            polls: self.polls_served.load(Ordering::Relaxed),
            walk_costs: self.walk_costs_served.load(Ordering::Relaxed),
            detour_sum: self.detour_sum.load(Ordering::Relaxed),
            detour_max: self.detour_max.load(Ordering::Relaxed),
            detour_plans: self.detour_plans.load(Ordering::Relaxed),
        }
    }

    /// Direct-read counters (no admission, no queue): batches served per op.
    /// Read beside `stats()`, never inside it: the orchestrator parses
    /// that struct field by field, additive JSON only.
    pub fn query_stats(&self) -> (u64, u64) {
        (
            self.door_defs_served.load(Ordering::Relaxed),
            self.tiles_flags_served.load(Ordering::Relaxed),
        )
    }

    /// Script-read counters: one batch served per op, same discipline.
    pub fn query_stats_scripts(&self) -> (u64, u64, u64, u64) {
        (
            self.static_locs_served.load(Ordering::Relaxed),
            self.f2p_served.load(Ordering::Relaxed),
            self.toll_fence_served.load(Ordering::Relaxed),
            self.wants_served.load(Ordering::Relaxed),
        )
    }

    /// Diag/bulk counters: one batch served per op, same discipline.
    pub fn query_stats_diag(&self) -> (u64, u64, u64, u64, u64) {
        (
            self.diff_served.load(Ordering::Relaxed),
            self.debug_served.load(Ordering::Relaxed),
            self.transitions_list_served.load(Ordering::Relaxed),
            self.probe_targets_served.load(Ordering::Relaxed),
            self.doors_near_served.load(Ordering::Relaxed),
        )
    }

    /// Operator reload (`reload-nav`): swap the snapshot atomically. The
    /// version bump makes every L1 key and in-flight join unreachable
    /// under the new version (keys carry it); requests in flight keep
    /// the old `Arc` and their replies carry the old version, so the
    /// worker rejects + resubmits once instead of serving stale.
    pub fn reload(&self, snapshot: Arc<Snapshot>) -> (String, String) {
        let mut guard = self.snapshot.lock().unwrap();
        let old = guard.version.clone();
        let new = snapshot.version.clone();
        *guard = snapshot;
        // New world, new baselines: old floors never price new geometry.
        self.best_costs.lock().unwrap().clear();
        (old, new)
    }

    /// L1 occupancy: entry count and stored bytes (RAM §5.9 evidence).
    pub fn l1_len(&self) -> usize {
        self.l1.lock().unwrap().map.len()
    }

    pub fn l1_bytes(&self) -> usize {
        self.l1.lock().unwrap().stored_bytes
    }

    /// Lot 4 operator metrics: queue depth now, in-flight tickets now, L1
    /// oldest entry age, daemon RSS, painted 64x64 squares. One lock each,
    /// never held during a Dijkstra (same discipline as the counters).
    /// Queue depth counts sent-but-undequeued jobs (a cancelled ticket
    /// still queued counts until the pool drops it: admission already
    /// released, so the cap never wedges on it).
    pub fn queue_depth(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    pub fn inflight_total(&self) -> usize {
        self.admission.lock().unwrap().values().sum()
    }

    pub fn l1_oldest_age_ms(&self) -> Option<u128> {
        self.l1
            .lock()
            .unwrap()
            .oldest_age()
            .map(|age| age.as_millis())
    }

    /// Daemon RSS in bytes (§5.9 enforcement reads this): /proc/self/statm
    /// field 2 is resident pages, 4 KiB each. Zero when unreadable (non-
    /// Linux or racing read): a missing gauge, never a plan verdict.
    pub fn rss_bytes() -> u64 {
        rss_bytes()
    }

    pub fn painted_squares(&self) -> usize {
        self.snapshot.lock().unwrap().world.painted_squares()
    }

    fn serve_one(&self, stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(ACCEPT_TIMEOUT));        let _ = stream.set_write_timeout(Some(ACCEPT_TIMEOUT));
        let mut line = String::new();
        let request = BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|error| error.to_string())
            .and_then(|_| json::parse(&line).map_err(|error| error.to_string()));
        // E3 fast lane: `plan` waits server-side on the bounded waiter
        // pool and answers terminal on the same connection — the 2 ms
        // poll loop never happens. The stream moves to the pool.
        if let Ok(ref request) = request {
            if request.get("op").as_str() == Some("plan") {
                self.plan_handoff(stream, request);
                return;
            }
        }
        let reply = match request {
            Ok(request) => self.dispatch(&request),
            Err(error) => object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&error)),
            ]),
        };
        let mut buf = json::stringify(&reply);
        buf.push('\n');
        let _ = (&stream).write_all(buf.as_bytes());
    }

    /// E3 handoff: the `plan` stream moves to the bounded waiter pool.
    /// A full waiter queue refuses loudly on the stream itself (the
    /// client falls back to submit/poll); the acceptor never blocks.
    fn plan_handoff(&self, stream: UnixStream, request: &Value) {
        let sender = self.wait_tx.lock().unwrap().clone();
        let Some(sender) = sender else {
            let reply = object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value("refused:wait-down")),
                ("version", str_value(&self.version())),
            ]);
            let mut buf = json::stringify(&reply);
            buf.push('\n');
            let _ = (&stream).write_all(buf.as_bytes());
            return;
        };
        let job = crate::service_plan::WaitJob { stream, request: request.clone() };
        if let Err(error) = sender.try_send(job) {
            // Full or disconnected: answer on the returned stream, the
            // client falls back to submit/poll. Never drop a stream
            // silently (the client would hang to its timeout).
            let mut job = match error {
                mpsc::TrySendError::Full(job) => job,
                mpsc::TrySendError::Disconnected(job) => job,
            };
            let reply = object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value("refused:wait-full")),
                ("version", str_value(&self.version())),
            ]);
            let mut buf = json::stringify(&reply);
            buf.push('\n');
            let _ = (&mut job.stream).write_all(buf.as_bytes());
        }
    }

    /// Waiter-pool submit: the terminal ticket id, or the refusal frame
    /// the client falls back on.
    pub(crate) fn op_submit_public(&self, request: &Value) -> Result<u64, Value> {
        let reply = self.op_submit(request);
        if reply.get("ok").as_bool() == Some(true) {
            match reply.get("id").as_i64() {
                Some(id) => Ok(id as u64),
                None => Err(reply),
            }
        } else {
            Err(reply)
        }
    }

    /// Waiter-pool poll: terminal frames or still pending. Unknown
    /// tickets surface as terminal refusals (the waiter answers and
    /// exits, never spins).
    pub(crate) fn op_poll_public(&self, id: u64) -> crate::service_plan::WaitPoll {
        use crate::service_plan::WaitPoll;
        let reply = self.poll_slot(&object(vec![("id", Value::Int(id as i64))]));
        if reply.get("ok").as_bool() != Some(true) {
            return WaitPoll::Terminal(reply);
        }
        let version = reply.get("version").clone();
        match reply.get("status").as_str().unwrap_or("") {
            "ready" => WaitPoll::Terminal(object(vec![
                ("ok", Value::Bool(true)),
                ("outcome", str_value("ready")),
                ("plan", reply.get("plan").clone()),
                ("version", version),
            ])),
            "failed" => WaitPoll::Terminal(object(vec![
                ("ok", Value::Bool(true)),
                ("outcome", str_value("failed")),
                ("error", reply.get("error").clone()),
                ("version", version),
            ])),
            _ => WaitPoll::Pending,
        }
    }

    /// Waiter-pool waiter release at terminal: drops only the waiter
    /// count (pool completion already cleaned tickets/inflight/admission;
    /// the Ready/Failed slot stays consumable for joins).
    pub(crate) fn op_release_waiter(&self, id: u64) {
        let mut waiters = self.waiters.lock().unwrap();
        match waiters.get(&id).copied() {
            Some(count) if count > 1 => {
                waiters.insert(id, count - 1);
            }
            _ => {
                waiters.remove(&id);
            }
        }
    }

    /// Served snapshot version for waiter replies.
    pub(crate) fn version_public(&self) -> String {
        self.version()
    }

    fn dispatch(&self, request: &Value) -> Value {
        match request.get("op").as_str().unwrap_or("") {
            "submit" => self.op_submit(request),
            "poll" => self.op_poll(request),
            "cancel" => self.op_cancel(request),
            "walk_costs" => self.op_walk_costs(request),
            "door_defs" => self.op_door_defs(request),
            "tiles_flags" => self.op_tiles_flags(request),
            "static_locs_near" => self.op_static_locs_near(request),
            "in_f2p" => self.op_in_f2p(request),
            "toll_fence" => self.op_toll_fence(request),
            "wants" => self.op_wants(request),
            "diff" => self.op_diff(request),
            "debug" => self.op_debug(request),
            "transitions_list" => self.op_transitions_list(request),
            "probe_targets" => self.op_probe_targets(request),
            "doors_near" => self.op_doors_near(request),
            "stats" => {
                let stats = self.stats();
                object(vec![
                    ("ok", Value::Bool(true)),
                    ("version", str_value(&self.version())),
                    ("hits", Value::Int(stats.hits as i64)),
                    ("trunc_hits", Value::Int(stats.trunc_hits as i64)),
                    ("suffix_hits", Value::Int(stats.suffix_hits as i64)),
                    ("misses", Value::Int(stats.misses as i64)),
                    ("joins", Value::Int(stats.joins as i64)),
                    ("l2_hits", Value::Int(stats.l2_hits as i64)),
                    ("l2_miss", Value::Int(stats.l2_miss as i64)),
                    ("evictions", Value::Int(stats.evictions as i64)),
                    ("l1_len", Value::Int(self.l1_len() as i64)),
                    ("l1_bytes", Value::Int(self.l1_bytes() as i64)),
                    ("l1_oldest_age_ms", match self.l1_oldest_age_ms() {
                        Some(age) => Value::Int(age.min(i64::MAX as u128) as i64),
                        None => Value::Null,
                    }),
                    ("queue_depth", Value::Int(self.queue_depth() as i64)),
                    ("inflight", Value::Int(self.inflight_total() as i64)),
                    ("refused_cap", Value::Int(stats.refused_cap as i64)),
                    ("refused_full", Value::Int(stats.refused_full as i64)),
                    ("refused_stale", Value::Int(stats.refused_stale as i64)),
                    ("unknown", Value::Int(stats.unknown as i64)),
                    ("submits", Value::Int(stats.submits as i64)),
                    ("polls", Value::Int(stats.polls as i64)),
                    ("walk_costs", Value::Int(stats.walk_costs as i64)),
                    ("detour_sum", Value::Int(stats.detour_sum as i64)),
                    ("detour_max", Value::Int(i64::from(stats.detour_max))),
                    ("detour_plans", Value::Int(stats.detour_plans as i64)),
                    ("door_defs", Value::Int(self.door_defs_served.load(Ordering::Relaxed) as i64)),
                    ("tiles_flags", Value::Int(self.tiles_flags_served.load(Ordering::Relaxed) as i64)),
                    ("static_locs_near", Value::Int(self.static_locs_served.load(Ordering::Relaxed) as i64)),
                    ("in_f2p", Value::Int(self.f2p_served.load(Ordering::Relaxed) as i64)),
                    ("toll_fence", Value::Int(self.toll_fence_served.load(Ordering::Relaxed) as i64)),
                    ("wants", Value::Int(self.wants_served.load(Ordering::Relaxed) as i64)),
                    ("diff", Value::Int(self.diff_served.load(Ordering::Relaxed) as i64)),
                    ("debug", Value::Int(self.debug_served.load(Ordering::Relaxed) as i64)),
                    ("transitions_list", Value::Int(self.transitions_list_served.load(Ordering::Relaxed) as i64)),
                    ("probe_targets", Value::Int(self.probe_targets_served.load(Ordering::Relaxed) as i64)),
                    ("doors_near", Value::Int(self.doors_near_served.load(Ordering::Relaxed) as i64)),
                    ("ram_warn", Value::Int(stats.ram_warn as i64)),
                    ("ram_shed", Value::Int(stats.ram_shed as i64)),
                    ("rss_bytes", Value::Int(Self::rss_bytes().min(i64::MAX as u64) as i64)),
                    ("painted_squares", Value::Int(self.painted_squares() as i64)),
                ])
            }
            other => object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("unknown op {other:?}"))),
            ]),
        }
    }

    fn version(&self) -> String {
        self.snapshot.lock().unwrap().version.clone()
    }

    fn op_submit(&self, request: &Value) -> Value {
        self.submits_served.fetch_add(1, Ordering::Relaxed);
        let version = self.version();
        let parsed = (|| -> Result<(String, Tile, Tile, NavOptions, String), String> {
            let account = request
                .get("account")
                .as_str()
                .ok_or_else(|| "account".to_string())?
                .to_string();
            let from = parse_tile(request.get("from"))?;
            let dest = parse_tile(request.get("dest"))?;
            let options = decode_options(request.get("options"))?;
            let client_version = request.get("version").as_str().unwrap_or("").to_string();
            Ok((account, from, dest, options, client_version))
        })();
        let (account, from, dest, options, client_version): (
            String,
            Tile,
            Tile,
            NavOptions,
            String,
        ) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        // §5.9 enforcement, read per submit (periodic at ~0.3 req/s/bot):
        // warn logs + halves L1, shed refuses through the existing Refuse
        // path (the caller surfaces `nav-down`, logged + counted worker-side).
        // 95 % abort of in-flight searches is NOT wired: shutdown by cancel
        // stays operator-driven (TODO(shedding), see §5.9).
        match ram_gate_for(Self::rss_bytes(), RAM_WARN_BYTES, RAM_SHED_BYTES) {
            RamGate::Shed => {
                self.ram_shed.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "pathfinding ram shed rss={}B: submit refused, nav-down",
                    Self::rss_bytes()
                );
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("refused:ram-shed")),
                    ("version", str_value(&version)),
                ]);
            }
            RamGate::Warn => {
                self.ram_warn.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "pathfinding ram warn rss={}B: L1 halved",
                    Self::rss_bytes()
                );
                self.l1.lock().unwrap().evict_half();
            }
            RamGate::Pass => {}
        }
        // LOT3 L1: the full key (options + current version) first, before
        // any admission accounting: a hit never consumes queue or cap.
        // Bind first: no guard survives into the reply path (lock order
        // is sequential everywhere, never nested).
        let key = PlanKey::of(from, dest, &options, &version);
        let hit = self.l1.lock().unwrap().get(&key);
        if let Some((is_plan, body)) = hit {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return self.instant_slot(is_plan, body);
        }
        // LOT3 truncation: dest on a cached plan serves the prefix.
        let tables = self.snapshot.lock().unwrap().tables.clone();
        let prefix = self
            .l1
            .lock()
            .unwrap()
            .truncate(&tables, from, dest, &options, &version);
        if let Some(prefix) = prefix {
            self.trunc_hits.fetch_add(1, Ordering::Relaxed);
            return self.instant_slot(true, prefix);
        }
        // N2 suffix: from on a cached plan serves the tail (replans
        // after a flight, mid-route joins) with no search either.
        let suffix = self
            .l1
            .lock()
            .unwrap()
            .suffix(&tables, from, dest, &options, &version);
        if let Some(suffix) = suffix {
            self.suffix_hits.fetch_add(1, Ordering::Relaxed);
            return self.instant_slot(true, suffix);
        }
        // LOT3 vol en cours: same full key already queued/computing ->
        // join it, never duplicate the Dijkstra. Sequential locks only
        // (lock order: inflight, results, waiters, never nested).
        if let Some(id) = self.inflight.lock().unwrap().get(&key).copied() {
            let pending =
                matches!(self.results.lock().unwrap().get(&id), Some(Slot::Pending { .. }));
            if pending {
                *self.waiters.lock().unwrap().entry(id).or_insert(1) += 1;
                self.joins.fetch_add(1, Ordering::Relaxed);
                return object(vec![
                    ("ok", Value::Bool(true)),
                    ("id", Value::Int(id as i64)),
                    ("version", str_value(&version)),
                ]);
            }
        }
        {
            let mut admission = self.admission.lock().unwrap();
            let in_flight = admission.get(&account).copied().unwrap_or(0);
            if in_flight >= ACCOUNT_CAP {
                self.refused_cap.fetch_add(1, Ordering::Relaxed);
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("refused:account-cap")),
                    ("version", str_value(&version)),
                ]);
            }
            admission.insert(account.clone(), in_flight + 1);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        // Slot BEFORE the queue: the pool must never dequeue a job whose
        // Pending slot is not yet visible (it would drop a live ticket).
        // A refused queue removes the slot again — never a dangling id.
        self.results
            .lock()
            .unwrap()
            .insert(id, Slot::Pending { cancel: Arc::clone(&cancel) });
        self.inflight.lock().unwrap().insert(key.clone(), id);
        self.tickets.lock().unwrap().insert(id, (key.clone(), account.clone()));
        self.waiters.lock().unwrap().insert(id, 1);
        let job = Job {
            id,
            account: account.clone(),
            key,
            from,
            dest,
            options,
            cancel,
        };
        if let Err(error) = self.queue_tx.try_send(job) {            // The job back: its key recovers the exact in-flight entry
            // (the tickets map is already cleared below, so it cannot).
            let job = match error {
                mpsc::TrySendError::Full(back) | mpsc::TrySendError::Disconnected(back) => back,
            };
            self.results.lock().unwrap().remove(&job.id);
            self.tickets.lock().unwrap().remove(&job.id);
            self.waiters.lock().unwrap().remove(&job.id);
            let mut inflight = self.inflight.lock().unwrap();
            if inflight.get(&job.key) == Some(&job.id) {
                inflight.remove(&job.key);
            }
            let mut admission = self.admission.lock().unwrap();
            let in_flight = admission.get(&job.account).copied().unwrap_or(1);
            admission.insert(job.account, in_flight.saturating_sub(1));
            self.refused_full.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value("refused:queue-full")),
                ("version", str_value(&version)),
            ]);
        }
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.misses.fetch_add(1, Ordering::Relaxed);
        object(vec![
            ("ok", Value::Bool(true)),
            ("id", Value::Int(id as i64)),
            ("version", str_value(&version)),
        ])
    }

    /// A cache hit answers as a fresh terminal ticket (poll reads it
    /// once, same shape as a computed plan, no queue involved).
    /// Reap one consumable terminal when `results` overflows: a polled
    /// (already consumed) terminal first, oldest id first so the choice
    /// is FIFO-deterministic; an unpolled one only when no polled
    /// terminal exists (the bound holds, never a pending). The waiter
    /// orphan goes down with its slot: without this, every evicted id
    /// leaked one `waiters` entry forever (`unknown-ticket` storm
    /// artefact). One lock at a time, never nested.
    /// Returns false when nothing is consumable (pendings only): the
    /// caller breaks instead of spinning.
    fn evict_one(&self) -> bool {
        let victim = self
            .results
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, slot)| matches!(slot, Slot::Ready { .. } | Slot::Failed { .. }))
            .min_by_key(|(id, slot)| {
                let consumed = matches!(slot, Slot::Ready { polled: true, .. } | Slot::Failed { polled: true, .. });
                (!consumed, **id)
            })
            .map(|(id, _)| *id);
        if let Some(id) = victim {
            self.results.lock().unwrap().remove(&id);
            self.waiters.lock().unwrap().remove(&id);
            true
        } else {
            false
        }
    }
    fn instant_slot(&self, is_plan: bool, body: Value) -> Value {
        let version = self.version();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.waiters.lock().unwrap().insert(id, 1);
        let mut results = self.results.lock().unwrap();
        if is_plan {
            results.insert(id, Slot::Ready { plan: body, polled: false });
        } else {
            results.insert(id, Slot::Failed { error: body, polled: false });
        }
        drop(results);
        while self.results.lock().unwrap().len() > RESULTS_CAP {
            if !self.evict_one() {
                break;
            }
        }
        object(vec![
            ("ok", Value::Bool(true)),
            ("id", Value::Int(id as i64)),
            ("version", str_value(&version)),
        ])
    }

    fn op_poll(&self, request: &Value) -> Value {
        self.polls_served.fetch_add(1, Ordering::Relaxed);
        self.poll_slot(request)
    }

    /// Poll core without the public gauge. The waiter pool (E3
    /// server-side wait) spins here every 10 ms, so counting it priced
    /// every `plan` at ~7 polls and read 1.4 polls/s with no client
    /// polling at all. Only client `poll` ops move the gauge now.
    fn poll_slot(&self, request: &Value) -> Value {
        let version = self.version();
        let id = match request.get("id").as_i64() {
            Some(id) => id as u64,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("poll needs id")),
                ])
            }
        };
        // LOT3 shared tickets: Ready/Failed answers clone out WITHOUT
        // consuming the slot (N waiters poll the same id); eviction by
        // RESULTS_CAP reaps them, polled ones first. Pending stays until
        // computed/cancelled. A first poll marks the terminal consumed so
        // the eviction prefers it over a never-polled one.
        let mut results = self.results.lock().unwrap();
        match results.get_mut(&id) {
            Some(Slot::Pending { .. }) => object(vec![
                ("ok", Value::Bool(true)),
                ("status", str_value("pending")),
                ("version", str_value(&version)),
            ]),
            Some(Slot::Ready { plan, polled }) => {
                *polled = true;
                let plan = plan.clone();
                object(vec![
                    ("ok", Value::Bool(true)),
                    ("status", str_value("ready")),
                    ("plan", plan),
                    ("version", str_value(&version)),
                ])
            }
            Some(Slot::Failed { error, polled }) => {
                *polled = true;
                let error = error.clone();
                object(vec![
                    ("ok", Value::Bool(true)),
                    ("status", str_value("failed")),
                    ("error", error),
                    ("version", str_value(&version)),
                ])
            }
            None => {
                self.unknown_polls.fetch_add(1, Ordering::Relaxed);
                object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("unknown-ticket")),
                    ("version", str_value(&version)),
                ])
            }
        }
    }

    fn op_cancel(&self, request: &Value) -> Value {
        let id = match request.get("id").as_i64() {
            Some(id) => id as u64,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("cancel needs id")),
                ])
            }
        };
        // LOT3 shared tickets: only the last waiter truly cancels; the
        // others keep polling the same id. The last cancel also releases
        // the account admission NOW (lot 4): a cancelled ticket must not
        // wedge the cap until the pool dequeues and drops it. Double
        // release is harmless (`release` on a missing key is a no-op) and
        // the over-admit window is one ticket for milliseconds.
        let last = {
            let mut waiters = self.waiters.lock().unwrap();
            match waiters.get(&id).copied() {
                Some(count) if count > 1 => {
                    waiters.insert(id, count - 1);
                    false
                }
                _ => {
                    waiters.remove(&id);
                    true
                }
            }
        };
        if last {
            if let Some(Slot::Pending { cancel }) = self.results.lock().unwrap().remove(&id) {
                cancel.store(true, Ordering::Relaxed);
            }
            if let Some((key, owner)) = self.tickets.lock().unwrap().remove(&id) {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.get(&key) == Some(&id) {
                    inflight.remove(&key);
                }
                self.release(&owner);
            }
        }
        object(vec![("ok", Value::Bool(true))])
    }

    /// Multi-target costs (N1): one Dijkstra from the requester, stopped
    /// when every target is reached. Direct read, no ticket, no queue:
    /// batches are small and nearby (spot pickers), so one bounded search
    /// on the acceptor answers faster than a ticket round-trip. Same
    /// version discipline as the other direct reads: the worker adopts
    /// the served version and retries once, never plans local.
    fn op_walk_costs(&self, request: &Value) -> Value {
        self.walk_costs_served.fetch_add(1, Ordering::Relaxed);
        let version = self.version();
        let parsed = match walk_costs::decode_request(request) {
            Ok(parsed) => parsed,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        if !parsed.version.is_empty() && !version.is_empty() && parsed.version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={} server={version}",
                        parsed.version
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        // Batch cache first: identical ranking windows repeat every cycle.
        let key = CostsKey::of(parsed.from, &parsed.dests, &parsed.options, &version);
        if let Some(found) = self.costs_cache.lock().unwrap().get(&key).cloned() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return walk_costs::encode_response(&walk_costs::WalkCostsResp { costs: found, version });
        }
        let Some(shared) = self.shared_edges(&snapshot) else {
            // The L2 builds infallibly here (no yield gate): all-None
            // keeps the caller on Manhattan order instead of inventing
            // costs.
            return walk_costs::encode_response(&walk_costs::WalkCostsResp {
                costs: vec![None; parsed.dests.len()],
                version,
            });
        };
        let found = costs::plan_costs_with(
            &snapshot.world,
            &snapshot.tables,
            parsed.from,
            &parsed.dests,
            &parsed.options,
            &shared,
        );
        // Bounded batch cache (cleared when full, never truth): the next
        // identical ranking window serves with no search.
        {
            let mut cache = self.costs_cache.lock().unwrap();
            if cache.len() >= COSTS_CACHE_CAP {
                cache.clear();
            }
            cache.insert(key, found.clone());
        }
        walk_costs::encode_response(&walk_costs::WalkCostsResp { costs: found, version })
    }

    /// Direct read, no ticket: defs for a batch of loc ids off the shared
    /// snapshot. Version skew refuses stale like a submit; the worker
    /// adopts the served version and retries once, never plans local.
    fn op_door_defs(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let locs = match service_queries::parse_door_defs_request(request) {
            Ok(locs) => locs,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let defs = service_queries::door_defs(&snapshot, &locs);
        self.door_defs_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_door_defs_reply(&defs, &version)
    }

    /// Direct read, no ticket: static flags for a batch of tiles off the
    /// shared snapshot (diagnosis, stepping). Same version discipline.
    fn op_tiles_flags(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let tiles = match service_queries::parse_tiles_flags_request(request) {
            Ok(tiles) => tiles,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let rows = service_queries::tiles_flags(&snapshot, &tiles);
        self.tiles_flags_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_tiles_flags_reply(&rows, &version)
    }

    /// Direct read, no ticket: static locs near a center tile, nearest
    /// first. Same version discipline as the other direct reads.
    fn op_static_locs_near(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let center = match service_queries::parse_static_locs_near_request(request) {
            Ok(center) => center,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let locs = service_queries::static_locs_near(&snapshot, center.0, center.1, center.2, center.3);
        self.static_locs_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_static_locs_near_reply(&locs, &version)
    }

    /// Direct read, no ticket: one F2P verdict per tile, in request
    /// order. Same version discipline.
    fn op_in_f2p(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let tiles = match service_queries::parse_tiles_flags_request(request) {
            Ok(tiles) => tiles,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let inside = service_queries::in_f2p_batch(&snapshot, &tiles);
        self.f2p_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_in_f2p_reply(&inside, &version)
    }

    /// Direct read, no ticket: nearest toll-gate tile on this level
    /// within the radius, or null. Same version discipline.
    fn op_toll_fence(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let center = match service_queries::parse_toll_fence_request(request) {
            Ok(center) => center,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let gate = service_queries::toll_fence(&snapshot, center.0, center.1, center.2, center.3);
        self.toll_fence_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_toll_fence_reply(gate, &version)
    }

    /// Direct read, no ticket: every tag a plan may require, sorted and
    /// distinct. The worker satisfies them from live state; the reply
    /// version keys its cache. Same stale discipline.
    fn op_wants(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let tags = service_queries::wants(&snapshot);
        self.wants_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_wants_reply(&tags, &version)
    }

    /// Direct read, no ticket: static/live mismatches for a dossier
    /// window. Same version discipline as the other direct reads.
    fn op_diff(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let cells = match service_queries::parse_diff_request(request) {
            Ok(cells) => cells,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let entries = service_queries::diff_cells(&snapshot, &cells);
        self.diff_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_diff_reply(&entries, &version)
    }

    /// Direct read, no ticket: flags here/dest, template locs near dest
    /// and the plan (or NoRoute) for the same inputs `travel_to` uses.
    /// The Dijkstra runs inline on the acceptor (admin-only, one caller):
    /// a slow exhaustive NoRoute still lands inside the poll guard.
    fn op_debug(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let (from, dest, options) = match service_queries::parse_debug_request(request) {
            Ok(parsed) => parsed,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let (flags_here, flags_dest, locs, defs) = service_queries::debug_static(&snapshot, &from, &dest);
        let outcome = plan::plan(&snapshot.world, &snapshot.tables, from, dest, &options);
        self.debug_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_debug_reply(flags_here, flags_dest, &locs, &defs, &outcome, &version)
    }

    /// Direct read, no ticket: every transition row, in table order.
    /// Same version discipline as the other direct reads.
    fn op_transitions_list(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let snapshot = self.snapshot.lock().unwrap().clone();
        let rows = service_queries::transitions_list(&snapshot);
        self.transitions_list_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_transitions_list_reply(&rows, &version)
    }

    /// Direct read, no ticket: stands and expects for the filtered rows,
    /// prober options. Same version discipline.
    fn op_probe_targets(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let filter = request.get("filter").as_str().map(str::to_string);
        let snapshot = self.snapshot.lock().unwrap().clone();
        let targets = service_queries::probe_targets(&snapshot, filter.as_deref());
        self.probe_targets_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_probe_targets_reply(&targets, &version)
    }

    /// Direct read, no ticket: template doors within the radius, nearest
    /// first. Same version discipline.
    fn op_doors_near(&self, request: &Value) -> Value {
        let version = self.version();
        let client_version = request.get("version").as_str().unwrap_or("");
        if !client_version.is_empty() && !version.is_empty() && client_version != version {
            self.refused_stale.fetch_add(1, Ordering::Relaxed);
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "refused:stale-version client={client_version} server={version}"
                    )),
                ),
                ("version", str_value(&version)),
            ]);
        }
        let center = match service_queries::parse_center_radius(request) {
            Ok(center) => center,
            Err(error) => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&error)),
                    ("version", str_value(&version)),
                ])
            }
        };
        let snapshot = self.snapshot.lock().unwrap().clone();
        let doors = service_queries::doors_near(&snapshot, center.0, center.1, center.2, center.3);
        self.doors_near_served.fetch_add(1, Ordering::Relaxed);
        service_queries::encode_static_locs_near_reply(&doors, &version)
    }

    /// Pool worker: one job at a time, snapshot cloned per job, no lock
    /// held during the Dijkstra. A cancelled ticket drops its outcome.
    /// LOT3: the L2 edge set is cached per data version (compute once,
    /// serve every search); the outcome lands in L1 under the full key
    /// (negatives included, Aborted never).
    fn serve_plans(&self) {
        loop {
            let job = {
                let queue = self.queue_rx.lock().unwrap();
                queue.recv().map_err(|_| "closed".to_string())
            };
            let Ok(job) = job else {
                break;
            };
            self.queued.fetch_sub(1, Ordering::Relaxed);
            if !self.results.lock().unwrap().contains_key(&job.id) {
                // Cancelled while queued: the canceller already dropped
                // the maps and released admission. Release only when we
                // still own them, never twice for one ticket (a double
                // release stole a live ticket's cap unit and let the
                // effective concurrency drift above ACCOUNT_CAP).
                if self.tickets.lock().unwrap().remove(&job.id).is_some() {
                    let mut inflight = self.inflight.lock().unwrap();
                    if inflight.get(&job.key) == Some(&job.id) {
                        inflight.remove(&job.key);
                    }
                    self.release(&job.account);
                }
                continue;
            }
            let snapshot = self.snapshot.lock().unwrap().clone();
            let shared = self.shared_edges(&snapshot);
            let cancel = Arc::clone(&job.cancel);
            let outcome = match shared {
                Some(shared) => plan::plan_yielding_with(
                    &snapshot.world,
                    &snapshot.tables,
                    job.from,
                    job.dest,
                    &job.options,
                    &shared,
                    &mut || !cancel.load(Ordering::Relaxed),
                ),
                None => Err(plan::aborted_no_route(job.from)),
            };
            // N8 detour, best-served baseline: the planner is deterministic,
            // so a repeated pair must cost exactly its cheapest served
            // cost. `detour` is the excess over that baseline (0 on
            // healthy traffic: straight walks, obstacle walks, door
            // clicks and transition rides all re-price identically);
            // only a regression — a worse plan for a known pair — moves
            // the gauge. Bounded like the costs cache: a full map clears
            // and re-baselines at zero, never spikes.
            if let Ok(plan) = &outcome {
                let mut best = self.best_costs.lock().unwrap();
                if best.len() >= BEST_COSTS_CAP {
                    best.clear();
                }
                let (detour, floor) = best_detour(best.get(&job.key).copied(), plan.cost);
                best.insert(job.key.clone(), floor);
                drop(best);
                self.detour_sum.fetch_add(u64::from(detour), Ordering::Relaxed);
                self.detour_plans.fetch_add(1, Ordering::Relaxed);
                self.detour_max.fetch_max(detour, Ordering::Relaxed);
            }
            // Encode + L1 first (no results lock held: lock order stays
            // sequential). A cancelled outcome still caches: same key
            // means the same plan, and the next submit hits it.
            let key = PlanKey::of(job.from, job.dest, &job.options, &snapshot.version);
            let (is_plan, body) = match &outcome {
                Ok(plan) => (true, encode_plan(plan)),
                Err(no) => (false, encode_noroute(no)),
            };
            // Negatives cache (the dearest hits); Aborted and
            // BudgetExhausted never do (they depend on stop and load).
            let cacheable = outcome.as_ref().err().is_none_or(|no| no.reason.cacheable());
            if cacheable {
                self.l1.lock().unwrap().insert(key, is_plan, body.clone());
            }
            // One critical section: Pending becomes Ready/Failed with no
            // poll-visible hole (a poll never sees a missing live ticket).
            // A missing slot means every waiter cancelled: drop the outcome.
            let mut results = self.results.lock().unwrap();
            if matches!(results.get(&job.id), Some(Slot::Pending { .. })) {
                if is_plan {
                    results.insert(job.id, Slot::Ready { plan: body, polled: false });
                } else {
                    results.insert(job.id, Slot::Failed { error: body, polled: false });
                }
                // Bound stored results: evict a consumable outcome (never
                // a pending); its owner already gave up and fell back sync.
                drop(results);
                while self.results.lock().unwrap().len() > RESULTS_CAP {
                    if !self.evict_one() {
                        break;
                    }
                }
            }
            // A cancelled ticket drops its outcome; a served one leaves
            // its terminal slot for every waiter to poll. Admission is
            // released exactly once per ticket: a cancel already released
            // it and dropped the maps, so only the owner still holding
            // the ticket entry releases here.
            {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.get(&job.key) == Some(&job.id) {
                    inflight.remove(&job.key);
                }
            }
            if self.tickets.lock().unwrap().remove(&job.id).is_some() {
                self.release(&job.account);
            }
        }
    }

    /// LOT3 L2: one edge set per data version, shared by every search.
    /// A reload drains the old version while the new one computes once.
    fn shared_edges(&self, snapshot: &Arc<Snapshot>) -> Option<Arc<SharedEdges>> {
        if let Some(shared) = self.l2.lock().unwrap().0.get(&snapshot.version).cloned() {
            self.l2_hits.fetch_add(1, Ordering::Relaxed);
            return Some(shared);
        }
        self.l2_miss.fetch_add(1, Ordering::Relaxed);
        // The L2 is the permissive superset (requires/tolls/F2P lifted):
        // identical for every job, so a default options fits all.
        let mut always = || true;
        let edges = plan::prepare_shared(&snapshot.world, &snapshot.tables, &NavOptions::default(), &mut always)?;
        let shared = Arc::new(edges);
        let mut l2 = self.l2.lock().unwrap();
        if !l2.0.contains_key(&snapshot.version) {
            l2.1.push(snapshot.version.clone());
            l2.0.insert(snapshot.version.clone(), Arc::clone(&shared));
            while l2.0.len() > L2_VERSIONS_CAP {
                let oldest = l2.1.remove(0);
                l2.0.remove(&oldest);
            }
        }
        Some(shared)
    }

    fn release(&self, account: &str) {
        let mut admission = self.admission.lock().unwrap();
        let in_flight = admission.get(account).copied().unwrap_or(1);
        if in_flight <= 1 {
            admission.remove(account);
        } else {
            admission.insert(account.to_string(), in_flight - 1);
        }
    }
}

/// N8 baseline core, pure: excess over the cheapest served cost for
/// the pair, and the new floor. First sighting and improvements read
/// 0 and re-baseline; a regression keeps the old floor so every bad
/// plan counts, not just the first.
fn best_detour(known: Option<u32>, cost: u32) -> (u32, u32) {
    match known {
        None => (0, cost),
        Some(floor) if cost <= floor => (0, cost),
        Some(floor) => (cost - floor, floor),
    }
}

/// Best-served baseline cap: pairs, not bytes (keys are small). A full
/// map clears and re-baselines at zero, like the costs cache.
const BEST_COSTS_CAP: usize = 4096;

// -- client ------------------------------------------------------------------

/// Non-blocking submit, non-blocking poll, cancel: the caller keeps its
/// own keepalive between polls (see `Bot::plan_travel`).
#[derive(Clone, Debug)]
pub struct PlanClient {
    sock: PathBuf,
    account: String,
}

#[derive(Debug)]
pub enum Submit {
    Accepted { id: u64, version: String },
    Refused { reason: String, version: String },
}

#[derive(Debug)]
pub enum Poll {
    Pending,
    Ready(Plan),
    Failed(NoRoute),
    Unknown,
}

impl PlanClient {
    pub fn new(sock: &Path, account: &str) -> Self {
        PlanClient {
            sock: sock.to_path_buf(),
            account: account.to_string(),
        }
    }

    pub(crate) fn account_name(&self) -> &str {
        &self.account
    }

    pub(crate) fn round_trip(&self, request: &Value, timeout: Duration) -> Result<Value, String> {
        let stream = UnixStream::connect(&self.sock)
            .map_err(|_| format!("pathfinding unreachable on {}", self.sock.display()))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| error.to_string())?;
        let mut buf = json::stringify(request);
        buf.push('\n');
        (&stream)
            .write_all(buf.as_bytes())
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        json::parse(&line).map_err(|error| error.to_string())
    }

    pub fn submit(
        &self,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
        version: &str,
    ) -> Result<Submit, String> {
        let reply = self.round_trip(
            &object(vec![
                ("op", str_value("submit")),
                ("account", str_value(&self.account)),
                ("from", tile_value(from)),
                ("dest", tile_value(dest)),
                ("options", encode_options(options)),
                ("version", str_value(version)),
            ]),
            SUBMIT_TIMEOUT,
        )?;
        if reply.get("ok").as_bool() != Some(true) {
            return Ok(Submit::Refused {
                reason: reply.get("error").as_str().unwrap_or("refused").to_string(),
                version: reply.get("version").as_str().unwrap_or("").to_string(),
            });
        }
        Ok(Submit::Accepted {
            id: reply.get("id").as_i64().ok_or("submit without id")? as u64,
            version: reply.get("version").as_str().unwrap_or("").to_string(),
        })
    }

    /// Poll one round-trip: the outcome plus the serving snapshot
    /// version (the worker rejects a stale-version answer + resubmits
    /// once, never serves it).
    pub fn poll(&self, id: u64) -> Result<(Poll, String), String> {
        let reply = self.round_trip(
            &object(vec![
                ("op", str_value("poll")),
                ("id", Value::Int(id as i64)),
            ]),
            POLL_TIMEOUT,
        )?;
        let version = reply.get("version").as_str().unwrap_or("").to_string();
        if reply.get("ok").as_bool() != Some(true) {
            return Ok((Poll::Unknown, version));
        }
        match reply.get("status").as_str().unwrap_or("") {
            "pending" => Ok((Poll::Pending, version)),
            "ready" => Ok((Poll::Ready(decode_plan(reply.get("plan"))?), version)),
            "failed" => Ok((Poll::Failed(decode_noroute(reply.get("error"))?), version)),
            _ => Ok((Poll::Unknown, version)),
        }
    }

    pub fn cancel(&self, id: u64) -> Result<(), String> {
        self.round_trip(
            &object(vec![
                ("op", str_value("cancel")),
                ("id", Value::Int(id as i64)),
            ]),
            POLL_TIMEOUT,
        )?;
        Ok(())
    }

    /// Multi-target costs (N1): one `walk_costs` round-trip serves the
    /// whole batch, order-preserving, `None` per unplannable target.
    /// `Ok` carries the serving version; `Err` carries the refusal
    /// reason plus the serving version, so the worker adopts it and
    /// retries once like a plan. Transport errors carry no version.
    pub fn walk_costs(
        &self,
        from: Tile,
        dests: &[Tile],
        options: &NavOptions,
        version: &str,
    ) -> Result<(Vec<Option<u32>>, String), (String, String)> {
        let reply = self
            .round_trip(
                &walk_costs::encode_request(&walk_costs::WalkCostsReq {
                    from,
                    dests: dests.to_vec(),
                    options: options.clone(),
                    version: version.to_string(),
                }),
                SUBMIT_TIMEOUT,
            )
            .map_err(|transport| (transport, String::new()))?;
        let served = reply.get("version").as_str().unwrap_or("").to_string();
        walk_costs::decode_response(&reply)
            .map(|resp| (resp.costs, served.clone()))
            .map_err(|refused| (refused, served))
    }

    /// Direct read: one round-trip, no ticket, no queue (door defs, tile
    /// flags). Same framing and timeout as a poll; the caller pins the
    /// version and resubmits once on skew, like a plan.
    pub fn request(&self, request: &Value) -> Result<Value, String> {
        self.round_trip(request, POLL_TIMEOUT)
    }
}

/// Unique socket path for tests (parallel-safe).
pub fn test_socket(prefix: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{id}.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    fn workspace_root() -> PathBuf {
        if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("../LifeScape");
            }
        }
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    fn setup() -> (Arc<StaticWorld>, Arc<Tables>) {
        let root = workspace_root();
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes =
            std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json"))
                .expect("gametypes");
        let overrides =
            std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json"))
                .expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = StaticWorld::open(&zip, defs).expect("static world");
        let zones =
            crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        (world, Arc::new(tables))
    }

    /// One world + one service for the whole lib-test binary: setup
    /// paints lazily, so six parallel cold worlds would serialize on
    /// `prepare_edges` (no deadline by design) and starve each other.
    /// Per-test accounts keep the cap/queue accounting disjoint.
    fn shared_world() -> (Arc<StaticWorld>, Arc<Tables>) {
        static WORLD: std::sync::OnceLock<(Arc<StaticWorld>, Arc<Tables>)> = std::sync::OnceLock::new();
        WORLD.get_or_init(setup).clone()
    }

    fn shared_service() -> Arc<Service> {
        static SERVICE: std::sync::OnceLock<Arc<Service>> = std::sync::OnceLock::new();
        SERVICE
            .get_or_init(|| {
                let (world, tables) = shared_world();
                Service::start(
                    Snapshot::new(world, tables, "test-version".to_string()),
                    &test_socket("plan-shared"),
                    PLAN_THREADS,
                )
                .expect("service starts")
            })
            .clone()
    }

    fn shared_client(account: &str) -> PlanClient {
        PlanClient::new(shared_service().socket_path(), account)
    }

    fn short_options() -> NavOptions {
        NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: HashSet::new(),
            excluded_edges: HashSet::new(),
            danger: Vec::new(),
        }
    }

    /// Poll until a terminal outcome or the bound (production mirror:
    /// bounded wait, then cancel + resubmit, never an unbounded hang).
    fn await_ready(client: &PlanClient, id: u64) -> Option<Poll> {
        let start = std::time::Instant::now();
        loop {
            match client.poll(id).expect("poll answers") {
                (Poll::Pending, _) => {
                    if start.elapsed() >= Duration::from_secs(25) {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                outcome => return Some(outcome.0),
            }
        }
    }

    /// Submit until a terminal outcome lands (Ready or Failed). Timeouts
    /// cancel and resubmit; `Failed` retries on the now-warmer world (a
    /// debug-cold budget flip resolves as squares paint). Returns the last
    /// verdict after 5 attempts: fast legs converge, a genuinely wrong
    /// engine fails every attempt identically.
    fn submit_terminal(client: &PlanClient, from: Tile, dest: Tile, options: &NavOptions, version: &str) -> Poll {
        let mut last = Poll::Unknown;
        for _ in 0..5 {
            let id = match client.submit(from, dest, options, version).expect("submit answers") {
                Submit::Accepted { id, .. } => id,
                Submit::Refused { reason, .. } => panic!("submit refused: {reason}"),
            };
            match await_ready(client, id) {
                Some(terminal @ (Poll::Ready(_) | Poll::Failed(_))) => {
                    last = terminal;
                    // A Failed verdict on a cold world may be a budget flip;
                    // retry warmer, but keep it: the caller asserts on it.
                    if matches!(last, Poll::Ready(_)) {
                        return last;
                    }
                }
                Some(Poll::Unknown) | Some(Poll::Pending) => {}
                None => {
                    let _ = client.cancel(id);
                }
            }
        }
        last
    }

    /// Submit until a Ready plan lands; a `Failed` verdict panics with its
    /// reason (use `submit_terminal` when either outcome is meaningful).
    fn submit_ready(client: &PlanClient, from: Tile, dest: Tile, options: &NavOptions, version: &str) -> Plan {
        match submit_terminal(client, from, dest, options, version) {
            Poll::Ready(plan) => plan,
            Poll::Failed(no) => panic!("no true verdict after warmup, last: {}", no.reason.as_str()),
            Poll::Unknown | Poll::Pending => panic!("poll lost the ticket"),
        }
    }

    #[test]
    fn codec_round_trip_is_exact() {
        let (world, tables) = shared_world();
        let from = Tile {
            x: 3228,
            z: 3214,
            level: 0,
        };
        let dest = Tile {
            x: 3224,
            z: 3214,
            level: 0,
        };
        let plan =
            plan::plan(&world, &tables, from, dest, &short_options()).expect("door leg plans");
        let again = decode_plan(&encode_plan(&plan)).expect("plan decodes");
        assert_eq!(
            json::stringify(&encode_plan(&again)),
            json::stringify(&encode_plan(&plan)),
            "plan codec is exact"
        );
        let no = plan::plan(
            &world,
            &tables,
            Tile {
                x: 3191,
                z: 3366,
                level: 0,
            },
            Tile {
                x: 3191,
                z: 3361,
                level: 0,
            },
            &short_options(),
        )
        .expect_err("guild door refuses without qp");
        let again = decode_noroute(&encode_noroute(&no)).expect("noroute decodes");
        assert_eq!(
            json::stringify(&encode_noroute(&again)),
            json::stringify(&encode_noroute(&no)),
            "noroute codec is exact"
        );
        let options = decode_options(&encode_options(&short_options())).expect("options decode");
        assert_eq!(options.allow_paid, short_options().allow_paid);
        assert_eq!(options.f2p_only, short_options().f2p_only);
    }

    /// Mixed versions: a body without the new fields decodes with safe
    /// defaults (shut door, never auto-paid, no recorded pick).
    #[test]
    fn codec_tolerates_missing_door_and_ride_fields() {
        let stand = Tile { x: 1, z: 0, level: 0 };
        let door_old = object(vec![
            ("kind", str_value("door")),
            (
                "door",
                object(vec![
                    ("loc", Value::Int(7)),
                    ("x", Value::Int(2)),
                    ("z", Value::Int(0)),
                    ("level", Value::Int(0)),
                ]),
            ),
            ("stand", tile_value(stand)),
            ("cross_to", tile_value(Tile { x: 3, z: 0, level: 0 })),
        ]);
        let ride_old = object(vec![
            ("kind", str_value("ride")),
            (
                "ride",
                ride_value(&crate::plan::TransitionRef {
                    name: "stairs".to_string(),
                    loc: 1,
                    climb_loc: 2,
                    op: 1,
                    from: Tile { x: 5, z: 5, level: 0 },
                    to: Tile { x: 5, z: 5, level: 1 },
                    footprint: vec![(5, 5)],
                    needs_choice: true,
                }),
            ),
            ("stand", tile_value(stand)),
            ("expect", tile_value(Tile { x: 5, z: 5, level: 1 })),
        ]);
        // Strip the new fields: the L1 shape the old daemon served.
        let mut ride_fields = ride_old.get("ride").as_obj().cloned().unwrap_or_default();
        ride_fields.remove("needs_choice");
        let ride_old = object(vec![
            ("kind", str_value("ride")),
            ("ride", Value::Obj(ride_fields)),
            ("stand", tile_value(stand)),
            ("expect", tile_value(Tile { x: 5, z: 5, level: 1 })),
        ]);
        let plan = decode_plan(&object(vec![
            ("cost", Value::Int(30)),
            ("segments", Value::Arr(vec![door_old, ride_old])),
        ]))
        .expect("old body decodes");
        match &plan.segments[0] {
            Segment::OpenDoor { door, .. } => {
                assert_eq!(door.open_op, None, "absent op never clicks blind");
                assert!(!door.toll, "absent toll never auto-pays");
            }
            other => panic!("want a door segment, got {other:?}"),
        }
        match &plan.segments[1] {
            Segment::Ride { transition, .. } => {
                assert!(!transition.needs_choice, "absent verdict names no pick");
            }
            other => panic!("want a ride segment, got {other:?}"),
        }
        // New bodies round-trip exactly, fields included.
        let (world, tables) = shared_world();
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let full = plan::plan(&world, &tables, from, dest, &short_options()).expect("door leg plans");
        let again = decode_plan(&encode_plan(&full)).expect("new body decodes");
        assert_eq!(
            json::stringify(&encode_plan(&again)),
            json::stringify(&encode_plan(&full)),
            "new codec is exact"
        );
        assert!(
            full.segments.iter().any(|segment| matches!(
                segment,
                Segment::OpenDoor { door, .. } if door.open_op.is_some()
            )),
            "the door leg carries its open op"
        );
        // Old NoRoute bodies predate "blockers": same verdict, no seal.
        let old_no = decode_noroute(&object(vec![
            ("reason", str_value("Disconnected")),
            ("closest", tile_value(Tile { x: 3077, z: 3429, level: 0 })),
            ("blocked", Value::Arr(Vec::new())),
            ("disabled", Value::Arr(Vec::new())),
        ]))
        .expect("old noroute decodes");
        assert!(old_no.blockers.is_empty(), "absent seal names nothing");
        assert_eq!(old_no.reason, NoRouteReason::Disconnected);
    }

    #[test]
    fn service_plan_matches_sync_on_golden_legs() {
        let client = shared_client("golden");
        let (world, tables) = shared_world();
        let legs = [
            (
                Tile {
                    x: 3228,
                    z: 3214,
                    level: 0,
                },
                Tile {
                    x: 3224,
                    z: 3214,
                    level: 0,
                },
                short_options(),
            ),
            (
                Tile {
                    x: 3228,
                    z: 3215,
                    level: 0,
                },
                Tile {
                    x: 3229,
                    z: 3212,
                    level: 2,
                },
                short_options(),
            ),
            (
                Tile {
                    x: 3224,
                    z: 3223,
                    level: 0,
                },
                Tile {
                    x: 3228,
                    z: 3226,
                    level: 0,
                },
                short_options(),
            ),
        ];
        for (from, dest, options) in legs {
            // Both engines warm up to the true verdict (see submit_ready).
            let plan = submit_ready(&client, from, dest, &options, "test-version");
            let mut last = "no attempt".to_string();
            let mut sync = None;
            for _ in 0..5 {
                match plan::plan(&world, &tables, from, dest, &options) {
                    Ok(found) => {
                        sync = Some(found);
                        break;
                    }
                    Err(no) => last = no.reason.as_str().to_string(),
                }
            }
            let sync = sync.unwrap_or_else(|| panic!("no true sync verdict, last: {last}"));
            assert_eq!(
                json::stringify(&encode_plan(&plan)),
                json::stringify(&encode_plan(&sync)),
                "service wraps plan(), never forks: ({},{},{})",
                from.x,
                from.z,
                from.level,
            );
        }
    }

    /// Isolation (§5.8, lot 2): same (from, dest), options differing by one
    /// field -> different keys, different wire requests, distinct correct
    /// plans. No cross-hit is possible: the key carries every per-bot field.
    #[test]
    fn isolation_no_cross_hit_between_option_sets() {
        let from = Tile {
            x: 3265,
            z: 3227,
            level: 0,
        };
        let dest = Tile {
            x: 3270,
            z: 3227,
            level: 0,
        };
        let base = short_options();
        let mut paid = base.clone();
        paid.allow_paid = true;
        let mut tagged = base.clone();
        tagged.satisfied.insert("coins:10".to_string());
        let mut banned = paid.clone();
        banned
            .excluded_edges
            .insert(EdgeId::Ride("toll_gate".to_string()));
        for (name, options) in [("paid", &paid), ("tagged", &tagged), ("banned", &banned)] {
            assert_ne!(
                PlanKey::of(from, dest, &base, "v"),
                PlanKey::of(from, dest, options, "v"),
                "{name} flips the key"
            );
            // The wire request carries every option field integrally.
            let wire = json::stringify(&encode_options(options));
            assert!(
                wire.contains("allow_paid")
                    && wire.contains("satisfied")
                    && wire.contains("excluded_edges"),
                "{name} fully serialized"
            );
        }
        assert_ne!(
            json::stringify(&encode_options(&base)),
            json::stringify(&encode_options(&paid)),
            "paid bit hits the wire"
        );
        // Behaviour on fast-both-sides legs (budget-proof under load):
        // same (from, dest), one option flipped -> a different outcome.
        let client = shared_client("iso");
        // Toll gate: paid rides straight through (cost 14).
        let toll = match submit_terminal(&client, from, dest, &paid, "test-version") {
            Poll::Ready(plan) => plan,
            other => panic!("toll side must plan, got {other:?}"),
        };
        assert_eq!(toll.cost, 14, "canonical toll path");
        // Free side: the 220-tile detour when warm, PaidOnly on a
        // debug-cold budget flip (documented §6 artefact) — either way it
        // is NOT the toll plan: no cross-hit.
        match submit_terminal(&client, from, dest, &base, "test-version") {
            Poll::Ready(plan) => assert_ne!(plan.cost, toll.cost, "free detour differs from toll"),
            Poll::Failed(no) => println!("free side flipped to {} under load, still not the toll plan", no.reason.as_str()),
            Poll::Unknown | Poll::Pending => panic!("poll lost the ticket"),
        }
        // Quest points: the guild door refuses without qp:32, opens with it.
        let guild_from = Tile { x: 3191, z: 3366, level: 0 };
        let guild_dest = Tile { x: 3191, z: 3361, level: 0 };
        match submit_terminal(&client, guild_from, guild_dest, &base, "test-version") {
            Poll::Failed(no) => assert_eq!(no.reason, NoRouteReason::NeedsRequirement, "guild door wants qp"),
            other => panic!("guild must refuse without qp, got {other:?}"),
        }
        let mut qp = base.clone();
        qp.satisfied.insert("qp:32".to_string());
        match submit_terminal(&client, guild_from, guild_dest, &qp, "test-version") {
            Poll::Ready(plan) => assert!(!plan.segments.is_empty(), "qp opens the guild door"),
            other => panic!("guild must open with qp:32, got {other:?}"),
        }
        // Excluded edge: ban the very door the base plan swings through.
        let door_from = Tile { x: 3228, z: 3214, level: 0 };
        let door_dest = Tile { x: 3224, z: 3214, level: 0 };
        let door_plan = submit_ready(&client, door_from, door_dest, &base, "test-version");
        let door_edge = door_plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                Segment::OpenDoor { door, .. } => Some(EdgeId::Door { loc: door.loc, x: door.x, z: door.z, level: door.level }),
                _ => None,
            })
            .expect("base plan swings a door");
        let mut banned_door = base.clone();
        banned_door.excluded_edges.insert(door_edge);
        match submit_terminal(&client, door_from, door_dest, &banned_door, "test-version") {
            Poll::Failed(no) => assert_eq!(no.reason, NoRouteReason::Disconnected, "banned door seals the pocket"),
            other => panic!("banned door must seal the pocket, got {other:?}"),
        }
    }

    #[test]
    fn stale_version_refuses_and_unknown_ticket_stays_unknown() {
        let client = shared_client("stale");
        match client
            .submit(
                Tile {
                    x: 3228,
                    z: 3214,
                    level: 0,
                },
                Tile {
                    x: 3224,
                    z: 3214,
                    level: 0,
                },
                &short_options(),
                "bogus",
            )
            .expect("stale answers")
        {
            Submit::Refused { reason, .. } => {
                assert!(reason.contains("stale"), "stale says stale, got {reason}")
            }
            Submit::Accepted { .. } => panic!("bogus version must not plan"),
        }
        assert!(
            matches!(client.poll(999_999).expect("poll").0, Poll::Unknown),
            "unknown ticket is unknown"
        );
        client.cancel(999_999).expect("cancel is idempotent");
    }

    #[test]
    fn down_socket_is_a_transport_error() {
        let client = PlanClient::new(&test_socket("absent"), "test");
        assert!(
            client
                .submit(
                    Tile {
                        x: 0,
                        z: 0,
                        level: 0
                    },
                    Tile {
                        x: 1,
                        z: 1,
                        level: 0
                    },
                    &short_options(),
                    ""
                )
                .is_err(),
            "no listener is an error, never a plan"
        );
    }

    /// IPC overhead vs the 5 ms lot-2 bar: sync compute on both sides is
    /// the same engine, so (service wall - sync wall) is the transport.
    /// Printed per leg with p50/p99; the hard assert is flake-proof, the
    /// release run below carries the verdict.
    #[test]
    fn ipc_overhead_measured_against_5ms_bar() {
        // Private queue on the shared world: no queue wait behind other
        // tests' jobs (timing domain), no second world load. One pool
        // thread: this service is the extra burner, keep it light.
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world.clone(), tables.clone(), "test-version".to_string()),
            &test_socket("plan-chrono"),
            1,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "chrono");
        let legs = [
            (
                Tile {
                    x: 3228,
                    z: 3214,
                    level: 0,
                },
                Tile {
                    x: 3224,
                    z: 3214,
                    level: 0,
                },
            ),
            (
                Tile {
                    x: 3224,
                    z: 3223,
                    level: 0,
                },
                Tile {
                    x: 3228,
                    z: 3226,
                    level: 0,
                },
            ),
            (
                Tile {
                    x: 3228,
                    z: 3215,
                    level: 0,
                },
                Tile {
                    x: 3229,
                    z: 3212,
                    level: 2,
                },
            ),
            (
                Tile {
                    x: 3097,
                    z: 3469,
                    level: 0,
                },
                Tile {
                    x: 3097,
                    z: 9868,
                    level: 0,
                },
            ),
            (
                Tile {
                    x: 3265,
                    z: 3227,
                    level: 0,
                },
                Tile {
                    x: 3270,
                    z: 3227,
                    level: 0,
                },
            ),
        ];
        let mut overheads = Vec::new();
        // Pure transport (acceptance number): submit/refused round-trips,
        // no planning involved — the framing cost alone, p99 vs the 5 ms bar.
        let mut transports = Vec::new();
        for _ in 0..50 {
            let start = std::time::Instant::now();
            match client.submit(legs[0].0, legs[0].1, &short_options(), "bogus") {
                Ok(Submit::Refused { .. }) => {}
                other => panic!("bogus version must refuse, got {other:?}"),
            }
            transports.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        transports.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rate = |q: f64| transports[((q * transports.len() as f64) as usize).min(transports.len() - 1)];
        println!("ipc transport ms: samples={} p50={:.2} p99={:.2} (bar 5ms)", transports.len(), rate(0.5), rate(0.99));
        assert!(rate(0.99) < 5.0, "ipc transport p99 above 5ms: {:.1}ms", rate(0.99));
        for (from, dest) in legs {
            // Warmup (untimed, verdict-agnostic): paint the world, then time.
            let _ = submit_terminal(&client, from, dest, &short_options(), "test-version");
            for _ in 0..4 {
                let sync_start = std::time::Instant::now();
                let sync = match plan::plan(&world, &tables, from, dest, &short_options()) {
                    Ok(found) => found,
                    Err(_) => continue,
                };
                let sync_ms = sync_start.elapsed().as_secs_f64() * 1000.0;
                let svc_start = std::time::Instant::now();
                let id = match client
                    .submit(from, dest, &short_options(), "test-version")
                    .expect("submit")
                {
                    Submit::Accepted { id, .. } => id,
                    Submit::Refused { reason, .. } => panic!("refused: {reason}"),
                };
                match await_ready(&client, id) {
                    Some(Poll::Ready(plan)) => {
                        assert_eq!(plan.cost, sync.cost, "same engine, same cost")
                    }
                    Some(_) | None => continue,
                }
                let svc_ms = svc_start.elapsed().as_secs_f64() * 1000.0;
                overheads.push((svc_ms - sync_ms).max(0.0));
                println!(
                    "leg ({},{})->({},{}) sync={sync_ms:.1}ms service={svc_ms:.1}ms",
                    from.x, from.z, dest.x, dest.z
                );
            }
        }
        overheads.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(!overheads.is_empty(), "no measurable round completed");
        let pick =
            |q: f64| overheads[((q * overheads.len() as f64) as usize).min(overheads.len() - 1)];
        println!(
            "ipc end-to-end ms: samples={} p50={:.2} p99={:.2} (diagnostic: compute jitter included, bar applies to transport above)",
            overheads.len(),
            pick(0.5),
            pick(0.99)
        );
        assert!(
            pick(0.99) < 500.0,
            "ipc overhead p99 absurdly high: {:.1}ms",
            pick(0.99)
        );
    }

    /// LOT3 L1: a repeated pair serves from cache (exact hit), and so
    /// does a repeated failure (negatives are the dearest hits: a
    /// Disconnected costs up to the 1.5 s budget).
    #[test]
    fn l1_caches_plans_and_negatives() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "lot3-l1".to_string()),
            &test_socket("plan-l1"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "l1");
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let first = submit_ready(&client, from, dest, &short_options(), "lot3-l1");
        assert!(!first.segments.is_empty(), "door leg plans");
        // Relative deltas only: warmup retries under load stay behind
        // the baseline, the counted submit is one hit, zero computes.
        let before = service.stats();
        let second = submit_ready(&client, from, dest, &short_options(), "lot3-l1");
        assert_eq!(
            json::stringify(&encode_plan(&first)),
            json::stringify(&encode_plan(&second)),
            "hit serves the same plan bit for bit"
        );
        let after_plans = service.stats();
        assert_eq!(after_plans.misses - before.misses, 0, "hit computes nothing");
        assert_eq!(after_plans.hits - before.hits, 1, "one exact hit");
        // Negative: the guild door refuses without qp:32, twice. Direct
        // submit (not the warming helper: a Failed verdict must count
        // exactly one compute + one hit).
        let guild_from = Tile { x: 3191, z: 3366, level: 0 };
        let guild_dest = Tile { x: 3191, z: 3361, level: 0 };
        let submit_once = |client: &PlanClient| match client.submit(guild_from, guild_dest, &short_options(), "lot3-l1") {
            Ok(Submit::Accepted { id, .. }) => id,
            Ok(Submit::Refused { reason, .. }) => panic!("refused: {reason}"),
            Err(error) => panic!("submit: {error}"),
        };
        for _ in 0..2 {
            let id = submit_once(&client);
            match await_ready(&client, id) {
                Some(Poll::Failed(no)) => assert_eq!(no.reason, NoRouteReason::NeedsRequirement),
                other => panic!("guild must refuse, got {other:?}"),
            }
        }
        let after = service.stats();
        assert_eq!(after.misses - after_plans.misses, 1, "negative computed once");
        assert_eq!(after.hits - after_plans.hits, 1, "negative served from L1");
        assert!(service.l1_len() >= 2, "plans + negatives share one LRU");
        println!(
            "l1: hits={} misses={} len={} bytes={}",
            after.hits,
            after.misses,
            service.l1_len(),
            service.l1_bytes()
        );
    }

    /// LOT3 vol en cours: the same full key twice while the Dijkstra
    /// runs returns ONE ticket (join), never two searches.
    #[test]
    fn inflight_join_shares_one_dijkstra() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "lot3-vol".to_string()),
            &test_socket("plan-vol"),
            1,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "vol");
        // Exhaustive NoRoute: the single pool thread stays busy past the
        // 1.5 s budget, so the twin submit always lands mid-flight.
        let from = Tile { x: 3219, z: 3218, level: 0 };
        let dest = Tile { x: 3215, z: 3218, level: 0 };
        let id = match client.submit(from, dest, &short_options(), "lot3-vol").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        let twin = match client.submit(from, dest, &short_options(), "lot3-vol").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("twin refused: {reason}"),
        };
        assert_eq!(id, twin, "same key in flight joins, never duplicates");
        let stats = service.stats();
        assert_eq!(stats.misses, 1, "one compute for two submits");
        assert_eq!(stats.joins, 1, "one join counted");
        // Both waiters cancel: the last one truly aborts the ticket.
        client.cancel(id).expect("cancel");
        client.cancel(twin).expect("cancel");
        // The service survives: a fast leg still lands afterwards.
        let fast = submit_ready(
            &client,
            Tile { x: 3228, z: 3214, level: 0 },
            Tile { x: 3224, z: 3214, level: 0 },
            &short_options(),
            "lot3-vol",
        );
        assert!(!fast.segments.is_empty(), "healthy after cancel");
    }

    /// LOT3 truncation: a dest ON a cached plan serves the prefix with no
    /// search, priced exactly like a fresh plan.
    #[test]
    fn truncation_serves_exact_prefix() {
        let (world, tables) = shared_world();
        // Pure unit shape first: a walk-only plan prefixes by tile count.
        let from = Tile { x: 0, z: 0, level: 0 };
        let tiles: Vec<Tile> = (1..=5).map(|x| Tile { x, z: 0, level: 0 }).collect();
        let walk = Plan {
            segments: vec![Segment::Walk { tiles: tiles.clone() }],
            cost: 5,
            adjusted_dest: None,
            dest_blocker: None,
        };
        let prefix = truncate_plan(&tables, from, &walk, Tile { x: 3, z: 0, level: 0 }).expect("on-path");
        assert_eq!(prefix.cost, 3, "walk prefix priced per tile");
        assert_eq!(plan_path(from, &prefix).last(), Some(&Tile { x: 3, z: 0, level: 0 }));
        assert!(truncate_plan(&tables, from, &walk, Tile { x: 9, z: 9, level: 0 }).is_none(), "off-path recomputes");
        // Through the service: full leg, then a mid-path dest.
        let service = Service::start(
            Snapshot::new(world.clone(), tables.clone(), "lot3-trunc".to_string()),
            &test_socket("plan-trunc"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "trunc");
        let door_from = Tile { x: 3228, z: 3214, level: 0 };
        let door_dest = Tile { x: 3224, z: 3214, level: 0 };
        let full = submit_ready(&client, door_from, door_dest, &short_options(), "lot3-trunc");
        let path = plan_path(door_from, &full);
        assert!(path.len() > 3, "door leg has a walk worth prefixing");
        let mid = path[path.len() / 2];
        let before = service.stats();
        let served = submit_ready(&client, door_from, mid, &short_options(), "lot3-trunc");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "prefix serves with no search");
        assert_eq!(after.trunc_hits - before.trunc_hits, 1, "one truncation counted");
        assert_eq!(plan_path(door_from, &served).last(), Some(&mid), "prefix lands on dest");
        // Exact price: a fresh sync plan for the same prefix costs the same.
        let mut sync = None;
        for _ in 0..5 {
            if let Ok(found) = plan::plan(&world, &tables, door_from, mid, &short_options()) {
                sync = Some(found);
                break;
            }
        }
        let sync = sync.expect("sync prefix converges");
        assert_eq!(served.cost, sync.cost, "truncated prefix is substructure-optimal");
    }

    /// LOT3 truncation, merged doors: `assemble` folds stepping on then
    /// straight off the same door (two Dijkstra edges, 2 x DOOR_COST) into
    /// one segment. The prefix must count per edge, not per segment —
    /// a merged prefix served 10 under the true 20 (same tiles, tight
    /// executor timeout, `served != sync`).
    #[test]
    fn truncation_counts_merged_door_per_edge() {
        let (_, tables) = shared_world();
        let from = Tile { x: 0, z: 0, level: 0 };
        let stand = Tile { x: 1, z: 0, level: 0 };
        let door_tile = Tile { x: 2, z: 0, level: 0 };
        let beyond = Tile { x: 3, z: 0, level: 0 };
        let door = crate::plan::DoorRef { loc: 7, x: 2, z: 0, level: 0, open_op: Some(1), toll: false };
        // Merged: cross_to past the door tile (two edges).
        let merged = Plan {
            segments: vec![
                Segment::Walk { tiles: vec![stand] },
                Segment::OpenDoor { door: door.clone(), stand, cross_to: beyond },
                Segment::Walk { tiles: vec![Tile { x: 4, z: 0, level: 0 }] },
            ],
            cost: 1 + 2 * plan::DOOR_COST + 1,
            adjusted_dest: None,
            dest_blocker: None,
        };
        let prefix = truncate_plan(&tables, from, &merged, beyond).expect("on-path");
        assert_eq!(prefix.cost, 1 + 2 * plan::DOOR_COST, "merged door counts two edges");
        // Single: stopping on the door tile (one edge).
        let single = Plan {
            segments: vec![
                Segment::Walk { tiles: vec![stand] },
                Segment::OpenDoor { door, stand, cross_to: door_tile },
            ],
            cost: 1 + plan::DOOR_COST,
            adjusted_dest: None,
            dest_blocker: None,
        };
        let prefix = truncate_plan(&tables, from, &single, door_tile).expect("on-path");
        assert_eq!(prefix.cost, 1 + plan::DOOR_COST, "single door counts one edge");
    }

    /// N1: one `walk_costs` op serves the whole batch in one search —
    /// order-preserving, one entry per target, `None` where unplannable.
    /// No plan ticket is consumed (misses do not move), the batch counter
    /// does. Stale versions refuse loud like every other direct read.
    #[test]
    fn walk_costs_op_serves_batch_in_one_search() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "costs-op".to_string()),
            &test_socket("plan-costs-op"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "costs");
        let from = Tile { x: 3230, z: 3241, level: 0 };
        let near = Tile { x: 3203, z: 3246, level: 0 };
        let far = Tile { x: 3228, z: 3214, level: 0 };
        let before = service.stats();
        let (found, served) = client
            .walk_costs(from, &[far, near, near], &short_options(), "costs-op")
            .expect("batch serves");
        assert_eq!(served, "costs-op");
        assert_eq!(found.len(), 3, "one cost per target, repeats included");
        assert_eq!(found[1], found[2], "repeats share one entry");
        assert!(found.iter().all(|cost| cost.is_some()), "all three walk, got: {found:?}");
        assert!(found[1] < found[0], "walk-cheap beats the Manhattan lure, got: {found:?}");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "no plan ticket consumed");
        assert_eq!(after.walk_costs - before.walk_costs, 1, "one batch counted");
        // Repeat batch: cached, no search, counted as a cache hit.
        let (again, _) = client
            .walk_costs(from, &[far, near, near], &short_options(), "costs-op")
            .expect("repeat serves");
        assert_eq!(again, found, "repeat serves identical costs");
        let cached = service.stats();
        assert_eq!(cached.walk_costs - before.walk_costs, 2, "each batch is one op");
        assert!(cached.hits > after.hits, "repeat batch serves from cache");
        // Same prices as the unit plans, without any ticket.
        let (world, tables) = shared_world();
        for (dest, cost) in [far, near].iter().zip([found[0], found[1]].iter()) {
            let mut sync = None;
            for _ in 0..5 {
                if let Ok(plan) = plan::plan(&world, &tables, from, *dest, &short_options()) {
                    sync = Some(plan.cost);
                    break;
                }
            }
            assert_eq!(*cost, sync, "batch prices the unit plan for ({},{})", dest.x, dest.z);
        }
        // Stale version refuses loud, never serves.
        let refused = client.walk_costs(from, &[near], &short_options(), "stale-v1");
        assert!(refused.is_err(), "stale version refuses, got: {refused:?}");
    }

    /// N2 suffix: standing mid-plan serves the tail with no search,
    /// priced exactly like a fresh plan from there.
    #[test]
    fn suffix_serves_tail_without_search() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world.clone(), tables.clone(), "lot3-suffix".to_string()),
            &test_socket("plan-suffix"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "suffix");
        // Unit shape first: a walk-only plan suffixes by tile count.
        let from = Tile { x: 0, z: 0, level: 0 };
        let tiles: Vec<Tile> = (1..=5).map(|x| Tile { x, z: 0, level: 0 }).collect();
        let walk = Plan {
            segments: vec![Segment::Walk { tiles: tiles.clone() }],
            cost: 5,
            adjusted_dest: None,
            dest_blocker: None,
        };
        let tail = split_suffix(&tables, from, &walk, Tile { x: 2, z: 0, level: 0 }).expect("mid-path");
        assert_eq!(tail.cost, 3, "dropped prefix priced per tile");
        assert_eq!(plan_path(Tile { x: 2, z: 0, level: 0 }, &tail).last(), Some(&Tile { x: 5, z: 0, level: 0 }));
        assert!(split_suffix(&tables, from, &walk, from).is_none(), "start is a prefix, not a suffix");
        assert!(split_suffix(&tables, from, &walk, Tile { x: 9, z: 9, level: 0 }).is_none(), "off-path recomputes");
        // Through the service: full leg, then a mid-path start.
        let door_from = Tile { x: 3228, z: 3214, level: 0 };
        let door_dest = Tile { x: 3224, z: 3214, level: 0 };
        let full = submit_ready(&client, door_from, door_dest, &short_options(), "lot3-suffix");
        let path = plan_path(door_from, &full);
        assert!(path.len() > 3, "door leg has a walk worth suffixing");
        let mid = path[path.len() / 2];
        let before = service.stats();
        let served = submit_ready(&client, mid, door_dest, &short_options(), "lot3-suffix");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 0, "suffix serves with no search");
        assert_eq!(after.suffix_hits - before.suffix_hits, 1, "one suffix counted");
        assert_eq!(plan_path(mid, &served).last(), Some(&door_dest), "suffix lands on dest");
        // Exact price: a fresh sync plan for the same tail costs the same.
        let mut sync = None;
        for _ in 0..5 {
            if let Ok(found) = plan::plan(&world, &tables, mid, door_dest, &short_options()) {
                sync = Some(found);
                break;
            }
        }
        let sync = sync.expect("sync suffix converges");
        assert_eq!(served.cost, sync.cost, "truncated tail is substructure-optimal");
    }

    /// Waiter-pool polls stay off the public gauge: `op_poll_public`
    /// (the E3 server-side wait, one poll per 10 ms slice) never moves
    /// `polls`; only a client `poll` op does. A `plan` round trip
    /// therefore costs one internal admission and zero polls.
    #[test]
    fn waiter_polls_stay_off_the_public_gauge() {
        use crate::service_plan::WaitPoll;
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "waiter-polls".to_string()),
            &test_socket("plan-waiter-polls"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "waiter-polls");
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let id = match client.submit(from, dest, &short_options(), "waiter-polls").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        let before = service.stats().polls;
        loop {
            match service.op_poll_public(id) {
                WaitPoll::Terminal(_) => break,
                WaitPoll::Pending => {}
            }
        }
        let _ = service.op_poll_public(id);
        let _ = service.op_poll_public(999_999);
        assert_eq!(service.stats().polls, before, "waiter polls stay off the gauge");
        let _ = client.poll(id).expect("poll answers");
        assert_eq!(service.stats().polls, before + 1, "one client poll counts once");
        client.cancel(id).expect("cancel");
    }

    /// N8 baseline core: first sighting and improvements read 0 and
    /// (re-)baseline; a regression reads the excess and keeps the old
    /// floor so every bad plan counts.
    #[test]
    fn best_detour_prices_regressions_only() {
        assert_eq!(super::best_detour(None, 13), (0, 13), "first sighting baselines");
        assert_eq!(super::best_detour(Some(13), 13), (0, 13), "identical re-price reads zero");
        assert_eq!(super::best_detour(Some(13), 9), (0, 9), "improvement re-baselines");
        assert_eq!(super::best_detour(Some(10), 13), (3, 10), "regression reads the excess");
        assert_eq!(super::best_detour(Some(0), 207), (207, 0), "worst case keeps the floor");
    }

    /// N8 acceptance: healthy traffic — straight walks, a door leg, an
    /// obstacle walk and a stairs ride — accumulates zero detour (every
    /// leg re-prices its own baseline), so the mean reads 0 <= 1.2.
    #[test]
    fn detour_metric_reads_zero_on_healthy_traffic() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "detour".to_string()),
            &test_socket("plan-detour"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "detour");
        // Straight walks, a door leg, an obstacle walk, a stairs ride:
        // every shape the old Chebyshev bound miscounted.
        let legs = [
            (Tile { x: 3220, z: 3218, level: 0 }, Tile { x: 3220, z: 3228, level: 0 }),
            (Tile { x: 3220, z: 3228, level: 0 }, Tile { x: 3220, z: 3218, level: 0 }),
            (Tile { x: 3228, z: 3214, level: 0 }, Tile { x: 3224, z: 3214, level: 0 }),
            (Tile { x: 3235, z: 3235, level: 0 }, Tile { x: 3245, z: 3235, level: 0 }),
            (Tile { x: 3228, z: 3215, level: 0 }, Tile { x: 3229, z: 3212, level: 2 }),
        ];
        let before = service.stats();
        for (from, dest) in legs {
            let outcome = submit_terminal(&client, from, dest, &short_options(), "detour");
            assert!(matches!(outcome, Poll::Ready(_)), "healthy leg plans");
        }
        let after = service.stats();
        assert_eq!(after.detour_plans - before.detour_plans, legs.len() as u64, "every computed plan counted");
        assert_eq!(after.detour_sum, before.detour_sum, "healthy traffic reads zero detour");
        let mean = (after.detour_sum - before.detour_sum) as f64 / legs.len() as f64;
        assert!(mean <= 1.2, "healthy mean <= 1.2, got {mean}");
    }

    /// LOT3 bump: a reloaded version serves zero stale plans — neither
    /// L1 hits nor in-flight joins cross the version.
    #[test]
    fn reload_bump_invalidates_cache_and_inflight() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world.clone(), tables.clone(), "lot3-old".to_string()),
            &test_socket("plan-bump"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "bump");
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let _ = submit_ready(&client, from, dest, &short_options(), "lot3-old");
        let _ = submit_ready(&client, from, dest, &short_options(), "lot3-old");
        let (old, new) = service.reload(Snapshot::new(world, tables, "lot3-new".to_string()));
        assert_ne!(old, new, "bump changes the version");
        // Same pair on the new version: a full miss, never a stale hit.
        // Relative deltas: warmup retries stay behind the baseline.
        let before = service.stats();
        let fresh = submit_ready(&client, from, dest, &short_options(), "lot3-new");
        assert!(!fresh.segments.is_empty(), "new version plans");
        let after = service.stats();
        assert_eq!(after.misses - before.misses, 1, "bump forces recompute");
        assert_eq!(after.hits - before.hits, 0, "zero stale hits across the bump");
        // Vol en cours across the bump: an old-version submit refuses
        // stale; a new-version twin joins the new flight, not the old one.
        let slow_from = Tile { x: 3219, z: 3218, level: 0 };
        let slow_dest = Tile { x: 3215, z: 3218, level: 0 };
        let id = match client.submit(slow_from, slow_dest, &short_options(), "lot3-new").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        match client.submit(slow_from, slow_dest, &short_options(), "lot3-old").expect("submit") {
            Submit::Refused { reason, .. } => assert!(reason.contains("stale"), "old vol refuses, got {reason}"),
            Submit::Accepted { .. } => panic!("old version must not join the new flight"),
        }
        let twin = match client.submit(slow_from, slow_dest, &short_options(), "lot3-new").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("twin refused: {reason}"),
        };
        assert_eq!(id, twin, "new vol joins the new flight");
        client.cancel(id).expect("cancel");
        client.cancel(twin).expect("cancel");
    }

    /// LOT3 acceptance: hit rate global >= 30 % on replayed pairs (§1.4:
    /// same legs come back in a loop). Method: N distinct pairs, 3 rounds
    /// each; round 1 misses, rounds 2-3 hit. Printed, asserted.
    #[test]
    fn hit_rate_benchmark_over_replayed_pairs() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "lot3-bench".to_string()),
            &test_socket("plan-bench"),
            2,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "bench");
        let mut qp = short_options();
        qp.satisfied.insert("qp:32".to_string());
        let mut paid = short_options();
        paid.allow_paid = true;
        let pairs: Vec<(Tile, Tile, NavOptions)> = vec![
            (Tile { x: 3228, z: 3214, level: 0 }, Tile { x: 3224, z: 3214, level: 0 }, short_options()),
            (Tile { x: 3228, z: 3215, level: 0 }, Tile { x: 3229, z: 3212, level: 2 }, short_options()),
            (Tile { x: 3224, z: 3223, level: 0 }, Tile { x: 3228, z: 3226, level: 0 }, short_options()),
            (Tile { x: 3265, z: 3227, level: 0 }, Tile { x: 3270, z: 3227, level: 0 }, paid),
            (Tile { x: 3191, z: 3366, level: 0 }, Tile { x: 3191, z: 3361, level: 0 }, qp),
            (Tile { x: 3224, z: 3223, level: 0 }, Tile { x: 3228, z: 3226, level: 0 }, short_options()),
        ];
        for _ in 0..3 {
            for (from, dest, options) in &pairs {
                match submit_terminal(&client, *from, *dest, options, "lot3-bench") {
                    Poll::Ready(_) | Poll::Failed(_) => {}
                    Poll::Unknown | Poll::Pending => panic!("poll lost the ticket"),
                }
            }
        }
        let stats = service.stats();
        let hits = stats.hits + stats.trunc_hits;
        let total = hits + stats.misses;
        let rate = 100.0 * hits as f64 / total as f64;
        println!(
            "hit-rate: pairs={} rounds=3 hits={} (exact={} trunc={}) joins={} misses={} rate={rate:.1}% (bar 30%) l2_hits={} l2_miss={} l1_len={} l1_bytes={}",
            pairs.len(),
            hits,
            stats.hits,
            stats.trunc_hits,
            stats.joins,
            stats.misses,
            stats.l2_hits,
            stats.l2_miss,
            service.l1_len(),
            service.l1_bytes(),
        );
        assert!(rate >= 30.0, "hit rate {rate:.1}% below the 30% bar");
        assert!(stats.l2_hits >= 1, "L2 shared past the first compute");
    }

    /// LOT3 acceptance: p99 reply <= 16 Ko (§5.3: max golden 9.6 Ko) and
    /// the L1 RAM line (1024 x max + overhead <= 11 Mo, §5.9).
    #[test]
    fn reply_size_and_ram_bound() {
        let (world, tables) = shared_world();
        let legs = [
            (Tile { x: 3228, z: 3214, level: 0 }, Tile { x: 3224, z: 3214, level: 0 }, short_options(), "door"),
            (Tile { x: 3228, z: 3215, level: 0 }, Tile { x: 3229, z: 3212, level: 2 }, short_options(), "stairs"),
            (Tile { x: 3265, z: 3227, level: 0 }, Tile { x: 3270, z: 3227, level: 0 }, short_options(), "toll-detour"),
            (Tile { x: 3025, z: 3512, level: 1 }, Tile { x: 3025, z: 3511, level: 1 }, short_options(), "bk-door"),
            (Tile { x: 3222, z: 3222, level: 0 }, Tile { x: 3250, z: 3420, level: 0 }, short_options(), "longue-marche"),
        ];
        let mut sizes = Vec::new();
        for (from, dest, options, name) in legs {
            let mut found = None;
            for _ in 0..5 {
                if let Ok(plan) = plan::plan(&world, &tables, from, dest, &options) {
                    found = Some(plan);
                    break;
                }
            }
            let Some(plan) = found else {
                // Debug-cold budget flip (documented §6 artefact): the
                // release run carries the verdict; debug reports the rest.
                println!("size: {name} skipped (cold budget flip, release covers it)");
                assert!(!cfg!(debug_assertions), "{name} must converge in release");
                continue;
            };
            let bytes = json::stringify(&encode_plan(&plan)).len();
            println!("size: {name} cost={} reply={bytes}B", plan.cost);
            sizes.push(bytes);
        }
        assert!(sizes.len() >= 4, "need at least 4 measured legs, got {}", sizes.len());
        sizes.sort_unstable();
        let max = *sizes.last().unwrap();
        let median = sizes[sizes.len() / 2];
        let p99 = sizes[((0.99 * sizes.len() as f64) as usize).min(sizes.len() - 1)];
        let l1_worst = 1024 * max + 350_000;
        println!(
            "reply: min={} median={median} max={max} p99={p99} (alert 16 Ko); L1 worst-case 1024x{max}+overhead = {l1_worst}B (line 11 Mo)",
            sizes[0]
        );
        assert!(p99 <= 16 * 1024, "p99 reply {p99}B above the 16 Ko alert");
        assert!(l1_worst <= 11 * 1024 * 1024, "L1 worst case {l1_worst}B above the 11 Mo line");
    }

    /// §5.9 enforcement trigger (E5): pure gate, injected thresholds, no
    /// real memory pressure. Constants match the doc lines (820/920 Mo of
    /// the 1 Go cap). The shed Refuse path itself is the existing one the
    /// queue-full tests already prove falls back sync client-side.
    #[test]
    fn ram_gate_fires_on_injected_thresholds() {
        assert_eq!(RAM_LIMIT_BYTES, 1024 * 1024 * 1024, "1 Go hard cap");
        assert_eq!(RAM_WARN_BYTES, 820 * 1024 * 1024, "doc warn line");
        assert_eq!(RAM_SHED_BYTES, 920 * 1024 * 1024, "doc shed line");
        assert_eq!(ram_gate_for(0, RAM_WARN_BYTES, RAM_SHED_BYTES), RamGate::Pass);
        assert_eq!(
            ram_gate_for(RAM_WARN_BYTES - 1, RAM_WARN_BYTES, RAM_SHED_BYTES),
            RamGate::Pass
        );
        assert_eq!(
            ram_gate_for(RAM_WARN_BYTES, RAM_WARN_BYTES, RAM_SHED_BYTES),
            RamGate::Warn
        );
        assert_eq!(
            ram_gate_for(RAM_SHED_BYTES - 1, RAM_WARN_BYTES, RAM_SHED_BYTES),
            RamGate::Warn
        );
        assert_eq!(
            ram_gate_for(RAM_SHED_BYTES, RAM_WARN_BYTES, RAM_SHED_BYTES),
            RamGate::Shed
        );
        assert_eq!(
            ram_gate_for(u64::MAX, RAM_WARN_BYTES, RAM_SHED_BYTES),
            RamGate::Shed
        );
    }

    /// L1 halve oldest-first (§5.9 warn): 4 entries in, oldest 2 out,
    /// newest 2 keep serving.
    #[test]
    fn l1_evict_half_keeps_newest() {
        let mut lru = Lru::new();
        let options = short_options();
        let keys: Vec<PlanKey> = (0..4)
            .map(|x| {
                PlanKey::of(
                    Tile { x, z: 0, level: 0 },
                    Tile { x: x + 10, z: 0, level: 0 },
                    &options,
                    "v",
                )
            })
            .collect();
        for key in &keys {
            assert!(lru.insert(key.clone(), true, Value::Bool(true)));
        }
        assert_eq!(lru.map.len(), 4, "four entries in");
        lru.evict_half();
        assert_eq!(lru.map.len(), 2, "oldest half out");
        assert!(!lru.map.contains_key(&keys[0]), "oldest evicted");
        assert!(!lru.map.contains_key(&keys[1]), "second oldest evicted");
        assert!(lru.map.contains_key(&keys[2]), "newest kept");
        assert!(lru.map.contains_key(&keys[3]), "newest kept");
    }

    /// Double cancel + cancel-after-completion (E6): every release is
    /// idempotent, the service keeps serving and the cap never wedges.
    #[test]
    fn double_cancel_and_cancel_after_completion_are_harmless() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "lot4-cancel".to_string()),
            &test_socket("plan-cancel2"),
            1,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "cancel2");
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        // Pending ticket cancelled twice: both answer ok, one release max.
        let id = match client.submit(from, dest, &short_options(), "lot4-cancel").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        client.cancel(id).expect("first cancel answers");
        client.cancel(id).expect("second cancel answers");
        // Completed ticket cancelled twice: terminal slot or not, still ok.
        let planned = match client.submit(from, dest, &short_options(), "lot4-cancel").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        match await_ready(&client, planned) {
            Some(Poll::Ready(plan)) => assert!(!plan.segments.is_empty(), "completion lands first"),
            other => panic!("fast leg must complete, got {other:?}"),
        }
        client.cancel(planned).expect("cancel after completion answers");
        client.cancel(planned).expect("second cancel after completion answers");
        // The cap never wedges and the service keeps serving afterwards.
        match client.submit(from, dest, &short_options(), "lot4-cancel").expect("submit") {
            Submit::Accepted { .. } => {}
            Submit::Refused { reason, .. } => panic!("cap wedged after double release: {reason}"),
        }
        let healthy = submit_ready(&client, from, dest, &short_options(), "lot4-cancel");
        assert!(!healthy.segments.is_empty(), "healthy after double cancel");
    }

    #[test]
    fn version_hash_bumps_on_content_change() {        let dir = std::env::temp_dir().join(format!(
            "lifescape-version-{}-{}",
            std::process::id(),
            NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.json");
        std::fs::write(&file, b"v1").unwrap();
        let first = data_version(std::slice::from_ref(&file));
        std::fs::write(&file, b"v2").unwrap();
        assert_ne!(
            first,
            data_version(std::slice::from_ref(&file)),
            "content change bumps the version"
        );
        assert_eq!(first.len(), 16, "opaque 16-hex version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    static NEXT_VERSION: AtomicU64 = AtomicU64::new(0);

    fn lot5_service() -> (Arc<Service>, PlanClient) {
        let (world, tables) = shared_world();
        let service = Service::start(Snapshot::new(world, tables, "lot5".to_string()), &test_socket("plan-lot5"), 1)
            .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "lot5");
        (service, client)
    }

    fn lot5_debug(client: &PlanClient, from: Tile, dest: Tile) -> Value {
        client
            .request(&object(vec![
                ("op", str_value("debug")),
                ("version", str_value("lot5")),
                (
                    "from",
                    object(vec![
                        ("x", Value::Int(i64::from(from.x))),
                        ("z", Value::Int(i64::from(from.z))),
                        ("level", Value::Int(i64::from(from.level))),
                    ]),
                ),
                (
                    "dest",
                    object(vec![
                        ("x", Value::Int(i64::from(dest.x))),
                        ("z", Value::Int(i64::from(dest.z))),
                        ("level", Value::Int(i64::from(dest.level))),
                    ]),
                ),
                ("options", encode_options(&short_options())),
            ]))
            .expect("debug answers")
    }

    /// L5 diag: `debug` plans the door leg inline, `diff` compares a
    /// worker window against the snapshot; both count server-side.
    #[test]
    fn diag_ops_diff_and_debug_serve_dossier_inputs() {
        let (service, client) = lot5_service();
        let before = service.query_stats_diag();
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let reply = lot5_debug(&client, from, dest);
        assert_eq!(reply.get("ok").as_bool(), Some(true), "debug answers, got {reply:?}");
        let answer = crate::service_queries::decode_debug_reply(&reply).expect("debug decodes");
        assert!(!answer.locs.is_empty(), "template locs near dest");
        assert!(answer.defs.iter().any(|def| def.open_op.is_some()), "a door def names its click");
        let plan = match answer.outcome {
            Ok(plan) => plan,
            Err(no) => panic!("door leg plans, got {}", no.reason.as_str()),
        };
        assert!(plan.segments.iter().any(|segment| matches!(segment, Segment::OpenDoor { .. })), "leg swings a door");
        // Diff: an exact cell agrees (no entry), a wrong id mismatches.
        let entry = &answer.locs[0];
        let cells = vec![
            crate::service_queries::DiffCell { x: entry.x, z: entry.z, level: entry.level, loc: Some((entry.loc, entry.shape, entry.angle)), removed: false },
            crate::service_queries::DiffCell { x: entry.x, z: entry.z, level: entry.level, loc: Some((entry.loc.wrapping_add(1), entry.shape, entry.angle)), removed: false },
        ];
        let reply = client
            .request(&object(vec![
                ("op", str_value("diff")),
                ("version", str_value("lot5")),
                ("cells", crate::service_queries::encode_diff_cells(&cells)),
            ]))
            .expect("diff answers");
        let diff = crate::service_queries::decode_diff_reply(&reply).expect("diff decodes");
        assert_eq!(diff.len(), 1, "only the wrong cell mismatches, got {diff:?}");
        assert_eq!(diff[0].static_loc, Some((entry.loc, entry.shape, entry.angle)), "static names the template");
        let after = service.query_stats_diag();
        assert_eq!(after.0 - before.0, 1, "diff counted");
        assert_eq!(after.1 - before.1, 1, "debug counted");
    }

    /// L5 diag: an unplannable dest answers NoRoute, never a plan.
    #[test]
    fn diag_ops_debug_reports_noroute() {
        let (_, client) = lot5_service();
        let reply = lot5_debug(&client, Tile { x: 3191, z: 3366, level: 0 }, Tile { x: 3191, z: 3361, level: 0 });
        let answer = crate::service_queries::decode_debug_reply(&reply).expect("debug decodes");
        match answer.outcome {
            Ok(_) => panic!("guild door must not plan without qp"),
            Err(no) => assert_eq!(no.reason, crate::plan::NoRouteReason::NeedsRequirement, "names the missing tags"),
        }
    }

    /// L5 discipline: stale refuses with the served version, bad payloads
    /// refuse typed, never a guess.
    #[test]
    fn diag_ops_stale_and_bad_payload() {
        let (_, client) = lot5_service();
        let reply = client
            .request(&object(vec![("op", str_value("diff")), ("version", str_value("bogus")), ("cells", Value::Arr(vec![]))]))
            .expect("stale answers");
        assert_eq!(reply.get("ok").as_bool(), Some(false), "stale refuses");
        assert_eq!(reply.get("version").as_str(), Some("lot5"), "served version rides along");
        for (op, body) in [
            ("diff", vec![("cells", Value::Int(1))]),
            ("debug", vec![]),
            ("doors_near", vec![("x", Value::Int(0))]),
        ] {
            let mut fields = vec![("op", str_value(op)), ("version", str_value("lot5"))];
            fields.extend(body);
            let reply = client.request(&object(fields)).expect("transport answers");
            assert_eq!(reply.get("ok").as_bool(), Some(false), "{op} refuses, got {reply:?}");
        }
    }

    /// L5 bulk: lists match the tables, stands match the planner, doors
    /// stay in radius and nearest-first.
    #[test]
    fn bulk_ops_lists_match_tables_and_prober() {
        let (service, client) = lot5_service();
        let (world, tables) = shared_world();
        let before = service.query_stats_diag();
        let reply = client
            .request(&object(vec![("op", str_value("transitions_list")), ("version", str_value("lot5"))]))
            .expect("list answers");
        let rows = crate::service_queries::decode_transitions_list_reply(&reply).expect("list decodes");
        assert_eq!(rows.len(), tables.transitions.len(), "every row ships");
        assert_eq!(rows[0].name, tables.transitions[0].name, "table order kept");
        let reply = client
            .request(&object(vec![("op", str_value("probe_targets")), ("version", str_value("lot5")), ("filter", Value::Null)]))
            .expect("targets answer");
        let targets = crate::service_queries::decode_probe_targets_reply(&reply).expect("targets decode");
        assert!(!targets.is_empty(), "unfiltered probes");
        let probed: Vec<_> = tables.transitions.iter().filter(|row| !row.disabled).collect();
        assert_eq!(targets.len(), probed.len(), "disabled excluded, rest kept");
        let options = NavOptions { allow_paid: true, f2p_only: true, satisfied: HashSet::new(), excluded_edges: HashSet::new(), danger: Vec::new() };
        for target in targets.iter().take(25) {
            let row = tables.transitions.iter().find(|row| row.name == target.view.name).expect("row exists");
            let direct = crate::plan::ride_stands(&world, &tables, &options, row).into_iter().next();
            assert_eq!(target.stand, direct, "{} stands like the planner", row.name);
            assert!(target.expect.is_some() == target.stand.is_some(), "stand and expect agree");
        }
        let reply = client
            .request(&object(vec![
                ("op", str_value("probe_targets")),
                ("version", str_value("lot5")),
                ("filter", str_value("zzz-no-such-row")),
            ]))
            .expect("filtered answers");
        assert!(crate::service_queries::decode_probe_targets_reply(&reply).expect("decodes").is_empty(), "no match, no rows");
        let (door_x, door_z, door_level): (i32, i32, i32) = (3226, 3223, 0);
        let door_loc = 1530u16;
        let reply = client
            .request(&object(vec![
                ("op", str_value("doors_near")),
                ("version", str_value("lot5")),
                ("x", Value::Int(i64::from(door_x))),
                ("z", Value::Int(i64::from(door_z))),
                ("level", Value::Int(i64::from(door_level))),
                ("radius", Value::Int(20)),
            ]))
            .expect("doors answer");
        let doors = crate::service_queries::decode_static_locs_near_reply(&reply).expect("doors decode");
        assert!(doors.iter().any(|entry| entry.loc == door_loc), "the door tile names its door");
        assert!(doors.windows(2).all(|pair| {
            let first = (pair[0].x - door_x).abs() + (pair[0].z - door_z).abs();
            let second = (pair[1].x - door_x).abs() + (pair[1].z - door_z).abs();
            first <= second
        }), "nearest first");
        assert!(doors.iter().all(|entry| (entry.x - door_x).abs() + (entry.z - door_z).abs() <= 20 && entry.level == door_level), "radius and level hold");
        let after = service.query_stats_diag();
        assert_eq!(after.2 - before.2, 1, "transitions_list counted");
        assert_eq!(after.3 - before.3, 2, "both probe batches counted");
        assert_eq!(after.4 - before.4, 1, "doors_near counted");
    }

    /// Cancel-then-drop must release admission exactly once: a cancelled
    /// queued job is dropped by the pool without touching the cap, so a
    /// live ticket of the same account stays counted until it completes.
    /// (Storm artefact: every cancel-then-drop stole one cap unit, the
    /// effective per-account concurrency drifted above ACCOUNT_CAP and
    /// fed the RESULTS_CAP churn behind `unknown-ticket`.)
    #[test]
    fn cancelled_queued_job_does_not_steal_admission() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "drop-adm".to_string()),
            &test_socket("plan-drop-adm"),
            1,
        )
        .expect("service starts");
        // Long leg (same as the operator-stop test): seconds of Dijkstra,
        // so ticket B is still in flight while A is already dropped.
        let from = Tile { x: 3222, z: 3222, level: 0 };
        let dest = Tile { x: 3250, z: 3420, level: 0 };
        let account = "drop-adm";
        let cancelled = match client_submit(&service, account, from, dest) {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        let client = PlanClient::new(service.socket_path(), account);
        client.cancel(cancelled).expect("cancel answers");
        let live = match client.submit(from, dest, &short_options(), "drop-adm").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("cap wedged by the cancel: {reason}"),
        };
        // While B is still in flight, the account holds exactly one unit:
        // A released once (cancel), never again (pool drop). Loop: the
        // drop lands milliseconds after the cancel, B runs for seconds.
        let start = std::time::Instant::now();
        loop {
            match client.poll(live).expect("poll answers") {
                (Poll::Pending, _) => {
                    let held = service.admission.lock().unwrap().get(account).copied();
                    assert_eq!(held, Some(1), "live ticket counted exactly once, got {held:?}");
                }
                (Poll::Ready(_), _) | (Poll::Failed(_), _) | (Poll::Unknown, _) => break,
            }
            assert!(start.elapsed() < Duration::from_secs(60), "long leg must terminate");
            std::thread::sleep(Duration::from_millis(5));
        }
        // B done: the cap is empty again, never negative, never stuck.
        assert_eq!(service.admission.lock().unwrap().get(account).copied(), None, "cap empty after completion");
    }

    /// Shared helper: submit on a fresh-service account (submit path
    /// without a client handle, for tests driving the maps directly).
    fn client_submit(service: &Service, account: &str, from: Tile, dest: Tile) -> Submit {
        let client = PlanClient::new(service.socket_path(), account);
        match client.submit(from, dest, &short_options(), "drop-adm").expect("submit") {
            Submit::Accepted { id, .. } => Submit::Accepted { id, version: "drop-adm".to_string() },
            Submit::Refused { reason, version } => Submit::Refused { reason, version },
        }
    }

    /// Eviction policy: polled (consumed) terminals first, oldest id
    /// first, an unpolled one only as a last resort, a pending never.
    /// Every reaped slot takes its waiter orphan down with it (the
    /// storm leaked one `waiters` entry per evicted id, unbounded).
    #[test]
    fn eviction_reaps_consumed_first_and_never_pendings() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "evict-pol".to_string()),
            &test_socket("plan-evict-pol"),
            1,
        )
        .expect("service starts");
        let terminal = |polled: bool| Slot::Ready { plan: object(vec![("k", str_value("v"))]), polled };
        {
            let mut results = service.results.lock().unwrap();
            let mut waiters = service.waiters.lock().unwrap();
            results.insert(7, terminal(true));
            results.insert(8, terminal(true));
            results.insert(9, terminal(false));
            results.insert(10, Slot::Pending { cancel: Arc::new(AtomicBool::new(false)) });
            for id in [7u64, 8, 9, 10] {
                waiters.insert(id, 1);
            }
        }
        assert!(service.evict_one(), "a consumable terminal exists");
        assert!(!service.results.lock().unwrap().contains_key(&7), "oldest consumed reaped first");
        assert!(service.results.lock().unwrap().contains_key(&8), "newer consumed waits its turn");
        assert!(service.results.lock().unwrap().contains_key(&9), "unconsumed survives while consumed exist");
        assert!(!service.waiters.lock().unwrap().contains_key(&7), "waiter orphan reaped with its slot");
        assert!(service.evict_one(), "one consumed left");
        assert!(!service.results.lock().unwrap().contains_key(&8), "second consumed gone");
        assert!(service.results.lock().unwrap().contains_key(&9), "unconsumed still survives");
        assert!(service.evict_one(), "last resort reaps the unconsumed");
        assert!(!service.results.lock().unwrap().contains_key(&9), "unconsumed falls last");
        assert!(!service.waiters.lock().unwrap().contains_key(&9), "orphan cleaned");
        assert!(!service.evict_one(), "pendings only: nothing to reap");
        let results = service.results.lock().unwrap();
        assert!(matches!(results.get(&10), Some(Slot::Pending { .. })), "pending never evicted");
        assert!(service.waiters.lock().unwrap().contains_key(&10), "pending waiter untouched");
    }

    /// Storm scenario at the protocol level: the slot disappears (here
    /// a last-waiter cancel, standing in for an eviction or a daemon
    /// restart) and the next poll is `unknown-ticket`, counted. A
    /// cancel on the missing id stays ok and leaves no waiter orphan:
    /// this is what the client now sends on every Unknown.
    #[test]
    fn cancelled_ticket_polls_unknown_and_cancel_cleans_orphan() {
        let (world, tables) = shared_world();
        let service = Service::start(
            Snapshot::new(world, tables, "cancel-unk".to_string()),
            &test_socket("plan-cancel-unk"),
            1,
        )
        .expect("service starts");
        let client = PlanClient::new(service.socket_path(), "cancel-unk");
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let id = match client.submit(from, dest, &short_options(), "cancel-unk").expect("submit") {
            Submit::Accepted { id, .. } => id,
            Submit::Refused { reason, .. } => panic!("refused: {reason}"),
        };
        client.cancel(id).expect("cancel answers");
        match client.poll(id).expect("poll answers") {
            (Poll::Unknown, _) => {}
            other => panic!("cancelled slot polls Unknown, got {other:?}"),
        }
        assert_eq!(service.stats().unknown, 1, "one unknown poll counted");
        client.cancel(id).expect("cancel on a missing id stays ok");
        assert!(!service.waiters.lock().unwrap().contains_key(&id), "no waiter orphan left");
        assert_eq!(service.stats().unknown, 1, "the cleanup cancel polls nothing");
    }
}
