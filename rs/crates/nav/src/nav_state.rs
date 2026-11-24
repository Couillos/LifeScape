#![forbid(unsafe_code)]
//! Navigation caches kept across restarts (lot N4, 2026-11-24).
//!
//! The route cache, the sealed-pocket verdicts and the list of painted
//! world squares are written at a clean shutdown and read back at the
//! next start, so a deploy no longer restarts with a cold service (12
//! restarts in 26 hours: after each, every plan was a miss until the
//! cache refilled, and every first touch of a world square painted it).
//!
//! The file is valid only for the data it was made from: it carries the
//! snapshot version (hash of the transition tables, overrides, loctypes,
//! free-to-play zones and the map pack) and this module's `FORMAT`. A
//! file that is absent, unreadable, of another format or of another data
//! version is ignored, never an error for the service: it starts cold
//! like before. `FORMAT` is also the knob for a planner change that keeps
//! the data but changes plans: bump it with that change and the old
//! file is dropped at the next start.
//!
//! Only what is expensive to recompute and exact to restore is kept:
//! positive routes (the plan, its options; the path analysis is
//! recomputed on load by the same `route_path::analyze` that fills the
//! cache live), pocket verdicts, painted squares. Negative verdicts live
//! 60 s and are never saved.

use std::path::Path;
use std::time::Instant;

use lifescape_json::{self as json, object, Value};

use crate::plan::{DangerZone, NavOptions};
use crate::route_cache::PlanEntry;
use crate::route_path;
use crate::service::{self, Service};

/// Layout and meaning of the file. Bump with any planner or cache change
/// that makes a saved plan differ from a freshly computed one.
pub const FORMAT: &str = "nav-state-1";

/// File size past which a state is not read (corrupt or runaway).
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// What a save or a restore moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub routes: usize,
    pub pockets: usize,
    pub squares: usize,
    pub millis: u128,
}

fn int(value: u64) -> Value {
    Value::Int(value.min(i64::MAX as u64) as i64)
}

fn entry_options(entry: &PlanEntry) -> NavOptions {
    NavOptions {
        allow_paid: entry.allow_paid,
        f2p_only: entry.f2p_only,
        satisfied: Default::default(),
        excluded_edges: entry.excluded.clone(),
        danger: entry.danger.clone(),
    }
}

impl Service {
    /// Write the caches to `path` (a temp file renamed over it, so a
    /// crash mid-write leaves the previous file whole).
    pub fn save_state(&self, path: &Path) -> Result<StateCounts, String> {
        let started = Instant::now();
        let (snapshot, routes, pockets) = self.state_rows();
        let squares = snapshot.world.painted_square_list();
        let route_rows: Vec<Value> = routes
            .iter()
            .map(|(id, stamp, entry)| {
                object(vec![
                    ("id", int(*id)),
                    ("stamp", int(*stamp)),
                    ("from", service::tile_value(entry.from)),
                    ("dest", service::tile_value(entry.dest)),
                    ("options", service::encode_options(&entry_options(entry))),
                    ("plan", service::encode_plan(&entry.plan)),
                ])
            })
            .collect();
        let pocket_rows: Vec<Value> = pockets
            .iter()
            .map(|(dest, members)| {
                let members = match members {
                    None => Value::Null,
                    Some(keys) => Value::Arr(keys.iter().map(|key| int(*key)).collect()),
                };
                Value::Arr(vec![int(*dest), members])
            })
            .collect();
        let document = object(vec![
            ("format", Value::Str(FORMAT.to_string())),
            ("version", Value::Str(snapshot.version.clone())),
            ("squares", Value::Arr(squares.iter().map(|(x, z)| Value::Arr(vec![Value::Int(i64::from(*x)), Value::Int(i64::from(*z))])).collect())),
            ("routes", Value::Arr(route_rows)),
            ("pockets", Value::Arr(pocket_rows)),
        ]);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json::stringify(&document)).map_err(|error| format!("write {}: {error}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|error| format!("rename {}: {error}", path.display()))?;
        Ok(StateCounts { routes: routes.len(), pockets: pockets.len(), squares: squares.len(), millis: started.elapsed().as_millis() })
    }

    /// Read the caches back from `path` into this service. `Err` carries
    /// the reason the file was ignored (absent, other format, other data
    /// version, unreadable); nothing is changed then.
    pub fn restore_state(&self, path: &Path) -> Result<StateCounts, String> {
        let started = Instant::now();
        let meta = std::fs::metadata(path).map_err(|error| format!("no state file ({error})"))?;
        if meta.len() > MAX_BYTES {
            return Err(format!("state file too large ({} bytes)", meta.len()));
        }
        let text = std::fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let document = json::parse(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
        if document.get("format").as_str() != Some(FORMAT) {
            return Err(format!("state format {:?}, wanted {FORMAT}", document.get("format").as_str()));
        }
        let snapshot = self.snapshot();
        let saved = document.get("version").as_str().unwrap_or("");
        if saved != snapshot.version {
            return Err(format!("state of data version {saved}, serving {}", snapshot.version));
        }
        // Decode everything before touching the service: a bad row drops
        // the whole file, never half a cache.
        let mut squares: Vec<(i32, i32)> = Vec::new();
        for pair in document.get("squares").as_arr().ok_or("squares")? {
            let pair = pair.as_arr().ok_or("square")?;
            let coord = |slot: usize| pair.get(slot).and_then(Value::as_i64).map(|value| value as i32).ok_or("square coordinate");
            squares.push((coord(0)?, coord(1)?));
        }
        let mut routes: Vec<(u64, PlanEntry)> = Vec::new();
        for row in document.get("routes").as_arr().ok_or("routes")? {
            let options = service::decode_options(row.get("options"))?;
            let plan = service::decode_plan(row.get("plan"))?;
            let from = service::parse_tile(row.get("from"))?;
            let dest = service::parse_tile(row.get("dest"))?;
            let stamp = row.get("stamp").as_i64().ok_or("stamp")? as u64;
            let danger: Vec<DangerZone> = options.danger.clone();
            let info = route_path::analyze(&snapshot.world, &snapshot.tables, from, &plan, &danger);
            routes.push((
                stamp,
                PlanEntry {
                    from,
                    dest,
                    plan,
                    f2p_only: options.f2p_only,
                    allow_paid: options.allow_paid,
                    excluded: options.excluded_edges,
                    danger,
                    info,
                },
            ));
        }
        let mut pockets: Vec<(u64, Option<Vec<u64>>)> = Vec::new();
        for row in document.get("pockets").as_arr().ok_or("pockets")? {
            let row = row.as_arr().ok_or("pocket row")?;
            let dest = row.first().and_then(Value::as_i64).ok_or("pocket dest")? as u64;
            let members = match row.get(1) {
                Some(Value::Null) | None => None,
                Some(list) => Some(list.as_arr().ok_or("pocket members")?.iter().filter_map(|key| key.as_i64().map(|key| key as u64)).collect()),
            };
            pockets.push((dest, members));
        }
        let counts = StateCounts { routes: routes.len(), pockets: pockets.len(), squares: squares.len(), millis: 0 };
        snapshot.world.warm_squares(&squares);
        self.restore_rows(&snapshot, routes, pockets);
        Ok(StateCounts { millis: started.elapsed().as_millis(), ..counts })
    }
}
