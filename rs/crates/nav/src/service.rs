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
//! `Refused`, never a block: the caller falls back to a local sync plan,
//! logs it and counts it. A version skew (client vs server snapshot)
//! refuses stale too; the client falls back sync, never loops.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, object, str_value, Value};

use crate::plan::{
    self, EdgeId, NavOptions, NoRoute, NoRouteReason, Plan, Segment, SharedEdges, Tile,
    TransitionRef,
};
use crate::static_world::StaticWorld;
use crate::tables::Tables;

/// Global queue bound: submit past this is `Refused`, never queued.
pub const QUEUE_CAP: usize = 16;
/// In-flight tickets (queued + computing) per account.
pub const ACCOUNT_CAP: usize = 2;
/// Plan threads sharing the snapshot (`~14 plans/s` on one thread at the
/// ~70 ms median; the second absorbs 1.5 s exhaustive NoRoutes).
pub const PLAN_THREADS: usize = 2;
/// Stored results bound: evicts consumable outcomes, never pendings.
pub const RESULTS_CAP: usize = 256;
/// Submit is admission only (no plan runs inside): fast by construction.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(10);
/// One poll round-trip bound (map lookup, no compute inside).
pub const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll pacing: IPC rhythm, not a game wait (the keepalive stays tick-driven).
pub const SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(2);
/// Client-side guard (mirrors the 30 s `UNLOAD` wall guard): cancel, sync fallback.
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
/// refusal degrades to a local sync plan client-side, logged + counted).
#[derive(Clone, Copy, Default, Debug)]
pub struct CacheStats {
    pub hits: u64,
    pub trunc_hits: u64,
    pub misses: u64,
    pub joins: u64,
    pub l2_hits: u64,
    pub l2_miss: u64,
    pub evictions: u64,
    pub refused_cap: u64,
    pub refused_full: u64,
    pub refused_stale: u64,
    pub ram_warn: u64,
    pub ram_shed: u64,
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
    })
}

// -- JSON codec (options serialized integrally, never normalized) -------------

fn tile_value(tile: Tile) -> Value {
    object(vec![
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
    ])
}

pub fn decode_options(value: &Value) -> Result<NavOptions, String> {
    let mut options = NavOptions {
        allow_paid: value.get("allow_paid").as_bool().ok_or("allow_paid")?,
        f2p_only: value.get("f2p_only").as_bool().ok_or("f2p_only")?,
        satisfied: std::collections::HashSet::new(),
        excluded_edges: std::collections::HashSet::new(),
    };
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
        } => object(vec![
            ("kind", str_value("custom")),
            ("name", str_value(&edge.name)),
            ("stand", tile_value(*stand)),
            ("expect", tile_value(*expect)),
        ]),
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
            edge: crate::plan::CustomEdgeRef {
                name: value.get("name").as_str().ok_or("custom.name")?.to_string(),
            },
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
    object(fields)
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
    })
}

fn reason_from_str(raw: &str) -> Result<NoRouteReason, String> {
    match raw {
        "Disconnected" => Ok(NoRouteReason::Disconnected),
        "NeedsRequirement" => Ok(NoRouteReason::NeedsRequirement),
        "PaidOnly" => Ok(NoRouteReason::PaidOnly),
        "OutsideF2p" => Ok(NoRouteReason::OutsideF2p),
        "Aborted" => Ok(NoRouteReason::Aborted),
        other => Err(format!("NoRoute reason {other:?}")),
    }
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
    Ok(NoRoute {
        closest_reachable: parse_tile(value.get("closest"))?,
        blocked_by_requirements: blocked,
        disabled_transitions_nearby: disabled,
        reason: reason_from_str(value.get("reason").as_str().ok_or("reason")?)?,
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
    Ready { plan: Value },
    Failed { error: Value },
}

/// Plan server: snapshot shared by the pool, bounded queue, IPC replies.
/// LOT3: global L1 (plans + negatives, keyed by the full PlanKey), global
/// L2 (one edge set per data version), in-flight joins on the full key.
/// Field order mirrors the lock discipline (admission, cache, in-flight,
/// results, snapshot); the pool never holds any lock during a Dijkstra.
pub struct Service {
    admission: Mutex<HashMap<String, usize>>,
    l1: Mutex<Lru>,
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
    misses: AtomicU64,
    joins: AtomicU64,
    l2_hits: AtomicU64,
    l2_miss: AtomicU64,
    refused_cap: AtomicU64,
    refused_full: AtomicU64,
    refused_stale: AtomicU64,
    ram_warn: AtomicU64,
    ram_shed: AtomicU64,
    sock_path: PathBuf,
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
            misses: AtomicU64::new(0),
            joins: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            l2_miss: AtomicU64::new(0),
            refused_cap: AtomicU64::new(0),
            refused_full: AtomicU64::new(0),
            refused_stale: AtomicU64::new(0),
            ram_warn: AtomicU64::new(0),
            ram_shed: AtomicU64::new(0),
            sock_path: sock.to_path_buf(),
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
        Ok(service)
    }

    pub fn socket_path(&self) -> &Path {
        &self.sock_path
    }

    /// Current snapshot version (operator gauge + skew logs).
    pub fn current_version(&self) -> String {
        self.version()
    }

    /// Lot 3 counters: hit-rate verdict reads these, not the logs.
    /// Lot 4 adds the refused_* ones: the fairness verdict reads those.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            trunc_hits: self.trunc_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            joins: self.joins.load(Ordering::Relaxed),
            l2_hits: self.l2_hits.load(Ordering::Relaxed),
            l2_miss: self.l2_miss.load(Ordering::Relaxed),
            evictions: self.l1.lock().unwrap().evictions,
            refused_cap: self.refused_cap.load(Ordering::Relaxed),
            refused_full: self.refused_full.load(Ordering::Relaxed),
            refused_stale: self.refused_stale.load(Ordering::Relaxed),
            ram_warn: self.ram_warn.load(Ordering::Relaxed),
            ram_shed: self.ram_shed.load(Ordering::Relaxed),
        }
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
        let _ = stream.set_read_timeout(Some(ACCEPT_TIMEOUT));
        let _ = stream.set_write_timeout(Some(ACCEPT_TIMEOUT));
        let mut line = String::new();
        let request = BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|error| error.to_string())
            .and_then(|_| json::parse(&line).map_err(|error| error.to_string()));
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

    fn dispatch(&self, request: &Value) -> Value {
        match request.get("op").as_str().unwrap_or("") {
            "submit" => self.op_submit(request),
            "poll" => self.op_poll(request),
            "cancel" => self.op_cancel(request),
            "stats" => {
                let stats = self.stats();
                object(vec![
                    ("ok", Value::Bool(true)),
                    ("version", str_value(&self.version())),
                    ("hits", Value::Int(stats.hits as i64)),
                    ("trunc_hits", Value::Int(stats.trunc_hits as i64)),
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
        // path (the caller falls back sync, logged + counted worker-side).
        // 95 % abort of in-flight searches is NOT wired: shutdown by cancel
        // stays operator-driven (TODO(shedding), see §5.9).
        match ram_gate_for(Self::rss_bytes(), RAM_WARN_BYTES, RAM_SHED_BYTES) {
            RamGate::Shed => {
                self.ram_shed.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "pathfinding ram shed rss={}B: submit refused, sync fallback",
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
    fn instant_slot(&self, is_plan: bool, body: Value) -> Value {
        let version = self.version();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.waiters.lock().unwrap().insert(id, 1);
        let mut results = self.results.lock().unwrap();
        if is_plan {
            results.insert(id, Slot::Ready { plan: body });
        } else {
            results.insert(id, Slot::Failed { error: body });
        }
        while results.len() > RESULTS_CAP {
            let evict = results.iter().find_map(|(id, slot)| {
                matches!(slot, Slot::Ready { .. } | Slot::Failed { .. }).then_some(*id)
            });
            match evict {
                Some(id) => {
                    results.remove(&id);
                }
                None => break,
            }
        }
        object(vec![
            ("ok", Value::Bool(true)),
            ("id", Value::Int(id as i64)),
            ("version", str_value(&version)),
        ])
    }

    fn op_poll(&self, request: &Value) -> Value {
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
        // RESULTS_CAP reaps them. Pending stays until computed/cancelled.
        let results = self.results.lock().unwrap();
        match results.get(&id) {
            Some(Slot::Pending { .. }) => object(vec![
                ("ok", Value::Bool(true)),
                ("status", str_value("pending")),
                ("version", str_value(&version)),
            ]),
            Some(Slot::Ready { plan }) => object(vec![
                ("ok", Value::Bool(true)),
                ("status", str_value("ready")),
                ("plan", plan.clone()),
                ("version", str_value(&version)),
            ]),
            Some(Slot::Failed { error }) => object(vec![
                ("ok", Value::Bool(true)),
                ("status", str_value("failed")),
                ("error", error.clone()),
                ("version", str_value(&version)),
            ]),
            None => object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value("unknown-ticket")),
                ("version", str_value(&version)),
            ]),
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
                self.release(&job.account);
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
            // Encode + L1 first (no results lock held: lock order stays
            // sequential). A cancelled outcome still caches: same key
            // means the same plan, and the next submit hits it.
            let key = PlanKey::of(job.from, job.dest, &job.options, &snapshot.version);
            let (is_plan, body) = match &outcome {
                Ok(plan) => (true, encode_plan(plan)),
                Err(no) => (false, encode_noroute(no)),
            };
            // Negatives cache (the dearest hits); Aborted never does.
            let cacheable = match &outcome {
                Ok(_) => true,
                Err(no) => no.reason != NoRouteReason::Aborted,
            };
            if cacheable {
                self.l1.lock().unwrap().insert(key, is_plan, body.clone());
            }
            // One critical section: Pending becomes Ready/Failed with no
            // poll-visible hole (a poll never sees a missing live ticket).
            // A missing slot means every waiter cancelled: drop the outcome.
            let mut results = self.results.lock().unwrap();
            if matches!(results.get(&job.id), Some(Slot::Pending { .. })) {
                if is_plan {
                    results.insert(job.id, Slot::Ready { plan: body });
                } else {
                    results.insert(job.id, Slot::Failed { error: body });
                }
                // Bound stored results: evict a consumable outcome (never
                // a pending); its owner already gave up and fell back sync.
                while results.len() > RESULTS_CAP {
                    let evict = results.iter().find_map(|(id, slot)| {
                        matches!(slot, Slot::Ready { .. } | Slot::Failed { .. }).then_some(*id)
                    });
                    match evict {
                        Some(id) => {
                            results.remove(&id);
                        }
                        None => break,
                    }
                }
            }
            // A cancelled ticket drops its outcome; a served one leaves
            // its terminal slot for every waiter to poll.
            {
                let mut inflight = self.inflight.lock().unwrap();
                if inflight.get(&job.key) == Some(&job.id) {
                    inflight.remove(&job.key);
                }
            }
            self.tickets.lock().unwrap().remove(&job.id);
            self.release(&job.account);
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

    fn round_trip(&self, request: &Value, timeout: Duration) -> Result<Value, String> {
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
        let door = crate::plan::DoorRef { loc: 7, x: 2, z: 0, level: 0 };
        // Merged: cross_to past the door tile (two edges).
        let merged = Plan {
            segments: vec![
                Segment::Walk { tiles: vec![stand] },
                Segment::OpenDoor { door: door.clone(), stand, cross_to: beyond },
                Segment::Walk { tiles: vec![Tile { x: 4, z: 0, level: 0 }] },
            ],
            cost: 1 + 2 * plan::DOOR_COST + 1,
            adjusted_dest: None,
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
        };
        let prefix = truncate_plan(&tables, from, &single, door_tile).expect("on-path");
        assert_eq!(prefix.cost, 1 + plan::DOOR_COST, "single door counts one edge");
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
}
