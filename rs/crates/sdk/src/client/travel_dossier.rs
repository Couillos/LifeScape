//! Failure dossiers (Phase 6, §6.4): what the bot saw when `travel_to`
//! gave up, written as JSON for `orchestrator replay-nav`.
//!
//! Pure JSON building plus the live-state gathering the dossier ships
//! (trail window, live locs, static/live diff, game messages). IO stays
//! here too (path choice, file write); the executor (`travel.rs`) only
//! calls `dossier_json` and `write_dossier_file` from `Bot::fail`.

use lifescape_json::{self as json, Value};
use lifescape_nav::service_queries::{self, DiffCell};
use lifescape_nav::plan::{EdgeId, NavOptions, Plan, Segment, Tile};

use crate::client::Bot;
use crate::error::TravelErrorKind;
use crate::waits::tick_ms;

/// Dossier field budgets (§6.4): trail marks, live window, messages.
pub(crate) const TRAIL_TAKE: usize = 50;
const DIFF_RADIUS: i32 = 10;
const MESSAGE_TAKE: usize = 20;

impl Bot {
    /// Live locs within the dossier window: dynamic state shadowing the
    /// served template (removed tiles ride along as `removed: true`). The
    /// window reads the live stream only, never painted static locs.
    pub(crate) fn live_locs(&self) -> Vec<Value> {
        let (here_x, here_z) = (self.world.x, self.world.z);
        let mut tiles: Vec<(i32, i32, i32)> = Vec::new();
        for tile in self.world.locs.keys() {
            let (x, z, level) = *tile;
            if (x - here_x).abs() <= DIFF_RADIUS && (z - here_z).abs() <= DIFF_RADIUS && !tiles.contains(&(x, z, level)) {
                tiles.push((x, z, level));
            }
        }
        tiles.sort();
        tiles
            .into_iter()
            .filter_map(|(x, z, level)| match self.world.loc_at(x, z, level) {
                Some((id, shape, angle)) => Some(json::object(vec![
                    ("x", Value::Int(i64::from(x))),
                    ("z", Value::Int(i64::from(z))),
                    ("level", Value::Int(i64::from(level))),
                    ("id", Value::Int(i64::from(id))),
                    ("shape", Value::Int(i64::from(shape))),
                    ("angle", Value::Int(i64::from(angle))),
                ])),
                None if self.world.loc_removed(x, z, level) => Some(json::object(vec![
                    ("x", Value::Int(i64::from(x))),
                    ("z", Value::Int(i64::from(z))),
                    ("level", Value::Int(i64::from(level))),
                    ("id", Value::Null),
                    ("removed", Value::Bool(true)),
                ])),
                None => None,
            })
            .collect()
    }

    /// Tiles where the live map disagrees with the served snapshot. The
    /// worker ships its live window (same tiles as `live_locs`, known
    /// state only) and the service compares against its template: opened
    /// doors, moved dynamics and missing data surface, an unstreamed
    /// stretch never fakes a B verdict. No handle or no answer reads
    /// empty (logged): never a local guess, the dossier still classifies
    /// A/C and the next plan replays the skew.
    pub(crate) fn static_live_diff(&self) -> Vec<Value> {
        let (here_x, here_z, here_level) = (self.world.x, self.world.z, self.world.level);
        let mut tiles: Vec<(i32, i32, i32)> = Vec::new();
        for tile in self.world.locs.keys() {
            let (x, z, level) = *tile;
            if level == here_level
                && (x - here_x).abs() <= DIFF_RADIUS
                && (z - here_z).abs() <= DIFF_RADIUS
                && !tiles.contains(&(x, z, level))
            {
                tiles.push((x, z, level));
            }
        }
        tiles.sort();
        let cells: Vec<DiffCell> = tiles
            .into_iter()
            .filter_map(|(x, z, level)| match self.world.loc_at(x, z, level) {
                Some((id, shape, angle)) => Some(DiffCell { x, z, level, loc: Some((id, shape, angle)), removed: false }),
                None if self.world.loc_removed(x, z, level) => Some(DiffCell { x, z, level, loc: None, removed: true }),
                None => None,
            })
            .collect();
        if cells.is_empty() {
            return Vec::new();
        }
        let Some(reply) = self.direct_best_effort("diff", vec![("cells", service_queries::encode_diff_cells(&cells))]) else {
            return Vec::new();
        };
        service_queries::decode_diff_reply(&reply).unwrap_or_default().into_iter().map(|entry| {
            let loc_value = |loc: Option<(u16, u8, u8)>| {
                loc.map_or(Value::Null, |(id, shape, angle)| {
                    json::object(vec![
                        ("loc", Value::Int(i64::from(id))),
                        ("shape", Value::Int(i64::from(shape))),
                        ("angle", Value::Int(i64::from(angle))),
                    ])
                })
            };
            json::object(vec![
                ("x", Value::Int(i64::from(entry.x))),
                ("z", Value::Int(i64::from(entry.z))),
                ("level", Value::Int(i64::from(entry.level))),
                ("static", loc_value(entry.static_loc)),
                ("live", loc_value(entry.live_loc)),
            ])
        }).collect()
    }

    /// Last game messages with their tick, oldest first.
    pub(crate) fn recent_messages(&self) -> Vec<Value> {
        let total = self.world.messages.len();
        let skip = total.saturating_sub(MESSAGE_TAKE);
        self.world.messages
            .iter()
            .zip(self.world.message_ticks.iter().chain(std::iter::repeat(&self.world.ticks)))
            .skip(skip)
            .map(|(text, tick)| {
                json::object(vec![("tick", Value::Int(*tick as i64)), ("text", Value::Str(text.clone()))])
            })
            .collect()
    }
}

pub(crate) fn trail_mark(mark: &(u64, i32, i32, i32)) -> Value {
    json::object(vec![
        ("tick", Value::Int(mark.0 as i64)),
        ("x", Value::Int(i64::from(mark.1))),
        ("z", Value::Int(i64::from(mark.2))),
        ("level", Value::Int(i64::from(mark.3))),
    ])
}

fn tile_value(tile: Tile) -> Value {
    json::object(vec![
        ("x", Value::Int(i64::from(tile.x))),
        ("z", Value::Int(i64::from(tile.z))),
        ("level", Value::Int(i64::from(tile.level))),
    ])
}

fn edge_value(edge: &EdgeId) -> Value {
    match edge {
        EdgeId::Door { loc, x, z, level } => json::object(vec![
            ("kind", Value::Str("door".to_string())),
            ("loc", Value::Int(i64::from(*loc))),
            ("x", Value::Int(i64::from(*x))),
            ("z", Value::Int(i64::from(*z))),
            ("level", Value::Int(i64::from(*level))),
        ]),
        EdgeId::Ride(name) => json::object(vec![("kind", Value::Str("ride".to_string())), ("name", Value::Str(name.clone()))]),
        EdgeId::Custom(name) => json::object(vec![("kind", Value::Str("custom".to_string())), ("name", Value::Str(name.clone()))]),
    }
}

fn segment_value(segment: &Segment) -> Value {
    match segment {
        Segment::Walk { tiles } => json::object(vec![
            ("kind", Value::Str("walk".to_string())),
            ("tiles", Value::Arr(tiles.iter().map(|tile| tile_value(*tile)).collect())),
        ]),
        Segment::OpenDoor { door, stand, cross_to } => json::object(vec![
            ("kind", Value::Str("door".to_string())),
            (
                "door",
                json::object(vec![
                    ("loc", Value::Int(i64::from(door.loc))),
                    ("x", Value::Int(i64::from(door.x))),
                    ("z", Value::Int(i64::from(door.z))),
                    ("level", Value::Int(i64::from(door.level))),
                ]),
            ),
            ("stand", tile_value(*stand)),
            ("cross_to", tile_value(*cross_to)),
        ]),
        Segment::Ride { transition, stand, pre_open, expect } => {
            let mut fields = vec![
                ("kind", Value::Str("ride".to_string())),
                ("name", Value::Str(transition.name.clone())),
                ("loc", Value::Int(i64::from(transition.loc))),
                ("climb_loc", Value::Int(i64::from(transition.climb_loc))),
                ("op", Value::Int(i64::from(transition.op))),
                ("from", tile_value(transition.from)),
                ("to", tile_value(transition.to)),
                ("stand", tile_value(*stand)),
                ("expect", tile_value(*expect)),
            ];
            if let Some(open) = pre_open {
                fields.push((
                    "pre_open",
                    json::object(vec![
                        ("loc", Value::Int(i64::from(open.loc))),
                        ("op", Value::Int(i64::from(open.op))),
                        ("open_to", Value::Int(i64::from(open.open_to))),
                    ]),
                ));
            }
            json::object(fields)
        }
        Segment::Custom { edge, stand, expect } => json::object(vec![
            ("kind", Value::Str("custom".to_string())),
            ("name", Value::Str(edge.name.clone())),
            ("stand", tile_value(*stand)),
            ("expect", tile_value(*expect)),
        ]),
    }
}

fn plan_value(plan: &Plan) -> Value {
    let mut fields = vec![
        ("cost", Value::Int(i64::from(plan.cost))),
        ("segments", Value::Arr(plan.segments.iter().map(segment_value).collect())),
    ];
    if let Some(adjusted) = plan.adjusted_dest {
        fields.push(("adjusted_dest", tile_value(adjusted)));
    }
    json::object(fields)
}

fn options_value(options: &NavOptions) -> Value {
    let mut satisfied: Vec<String> = options.satisfied.iter().cloned().collect();
    satisfied.sort();
    let mut banned: Vec<(&str, &EdgeId)> = Vec::new();
    for edge in &options.excluded_edges {
        let key = match edge {
            EdgeId::Door { .. } => "door",
            EdgeId::Ride(_) => "ride",
            EdgeId::Custom(_) => "custom",
        };
        banned.push((key, edge));
    }
    banned.sort_by(|left, right| left.0.cmp(right.0));
    json::object(vec![
        ("allow_paid", Value::Bool(options.allow_paid)),
        ("f2p_only", Value::Bool(options.f2p_only)),
        ("satisfied", Value::Arr(satisfied.into_iter().map(Value::Str).collect())),
        ("excluded_edges", Value::Arr(banned.into_iter().map(|(_, edge)| edge_value(edge)).collect())),
    ])
}

/// Pure dossier document (§6.4): everything `replay-nav` needs, no live
/// reads. IO (path choice, write, log) stays in `Bot::fail`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dossier_json(
    account: &str,
    tick: u64,
    from: Tile,
    dest: Tile,
    options: &NavOptions,
    plan: Option<&Plan>,
    failed_segment: Option<usize>,
    kind: TravelErrorKind,
    expected: &str,
    observed: &str,
    trail: &[Value],
    live_locs: &[Value],
    diff: &[Value],
    messages: &[Value],
) -> Value {
    let mut fields = vec![
        ("account", Value::Str(account.to_string())),
        ("tick", Value::Int(tick as i64)),
        ("tick_ms", Value::Int(tick_ms() as i64)),
        ("from", tile_value(from)),
        ("dest", tile_value(dest)),
        ("options", options_value(options)),
        ("failed_segment", failed_segment.map(|index| Value::Int(index as i64)).unwrap_or(Value::Null)),
        (
            "error",
            json::object(vec![
                ("kind", Value::Str(kind.as_str().to_string())),
                ("expected", Value::Str(expected.to_string())),
                ("observed", Value::Str(observed.to_string())),
            ]),
        ),
        ("trail", Value::Arr(trail.to_vec())),
        ("live_locs", Value::Arr(live_locs.to_vec())),
        ("static_live_diff", Value::Arr(diff.to_vec())),
        ("messages", Value::Arr(messages.to_vec())),
    ];
    fields.push(("plan", plan.map(plan_value).unwrap_or(Value::Null)));
    json::object(fields)
}

/// Dossier directory: `LIFESCAPE_ROOT` wins, else the first ancestor of
/// the working dir or the executable holding `orchestration` (same rule
/// as the orchestrator paths, without depending on it).
fn failures_dir() -> std::path::PathBuf {
    if let Ok(root) = std::env::var("LIFESCAPE_ROOT") {
        if !root.is_empty() {
            return std::path::PathBuf::from(root).join("run").join("failures");
        }
    }
    let mut candidates = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe;
        for _ in 0..6 {
            if let Some(parent) = dir.parent() {
                candidates.push(parent.to_path_buf());
                dir = parent.to_path_buf();
            }
        }
    }
    for dir in candidates {
        let mut probe = dir.clone();
        loop {
            if probe.join("orchestration").is_dir() {
                return probe.join("run").join("failures");
            }
            match probe.parent() {
                Some(parent) => probe = parent.to_path_buf(),
                None => break,
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join("run").join("failures")
}

/// Write one dossier file. `None` on any IO error: the caller logs it and
/// the error stands without a path.
pub(crate) fn write_dossier_file(account: &str, tick: u64, value: &Value) -> Option<std::path::PathBuf> {
    let dir = failures_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{account}-{tick}.json"));
    std::fs::write(&path, json::stringify(value)).ok()?;
    Some(path)
}
