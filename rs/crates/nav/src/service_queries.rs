//! Direct-read ops for workers without a map: door defs and tile flags.
//!
//! Same framing as the plan ops (JSON over the daemon socket) but no
//! admission, no queue, no ticket: the handler answers from the shared
//! snapshot, one lock at a time, and every reply carries the serving
//! version so the worker pins or resubmits exactly like a plan. Replies
//! follow request order; unknown loc ids decode to shut, never-paid
//! defaults, never an error.

use std::collections::HashSet;

use lifescape_json::{object, str_value, Value};

use crate::plan::{NavOptions, NoRoute, Plan, Tile};
use crate::tables::Transition;
use crate::service::Snapshot;

/// Static def for one loc id: the click an off-plan door needs plus the
/// footprint fields a holder needs once live supplies shape and angle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorDef {
    pub loc: u16,
    pub open_op: Option<u8>,
    pub toll: bool,
    pub blockwalk: bool,
    pub blockrange: bool,
    pub length: i32,
    pub width: i32,
    pub active: i32,
}

/// Static read for one tile: flags, recorded door, toll and static loc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileFlags {
    pub tile: Tile,
    pub flags: u32,
    pub door: Option<(u16, u8, u8)>,
    pub toll: bool,
    pub loc: Option<(u16, u8, u8)>,
}

/// Defs for a batch of loc ids, in request order. Unknown ids read as
/// shut, never-paid, zero footprint: the caller leaves them shut.
pub fn door_defs(snapshot: &Snapshot, loc_ids: &[u16]) -> Vec<DoorDef> {
    loc_ids
        .iter()
        .map(|loc| {
            let def = snapshot.world.loc_def(usize::from(*loc));
            DoorDef {
                loc: *loc,
                open_op: def.and_then(|def| def.open_op),
                toll: def.is_some_and(|def| def.toll),
                blockwalk: def.is_some_and(|def| def.blockwalk),
                blockrange: def.is_some_and(|def| def.blockrange),
                length: def.map(|def| def.length).unwrap_or(0),
                width: def.map(|def| def.width).unwrap_or(0),
                active: def.map(|def| def.active).unwrap_or(0),
            }
        })
        .collect()
}

/// Static reads for a batch of tiles, in request order. One lock per tile
/// read (same discipline as the plan path, never two held at once).
pub fn tiles_flags(snapshot: &Snapshot, tiles: &[Tile]) -> Vec<TileFlags> {
    tiles
        .iter()
        .map(|tile| TileFlags {
            tile: *tile,
            flags: snapshot.world.get(tile.x, tile.z, tile.level),
            door: snapshot.world.door_at(tile.x, tile.z, tile.level),
            toll: snapshot.world.is_toll(tile.x, tile.z, tile.level),
            loc: snapshot.world.loc_at(tile.x, tile.z, tile.level),
        })
        .collect()
}

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

fn triplet_value((loc, shape, angle): (u16, u8, u8)) -> Value {
    object(vec![
        ("loc", Value::Int(i64::from(loc))),
        ("shape", Value::Int(i64::from(shape))),
        ("angle", Value::Int(i64::from(angle))),
    ])
}

fn parse_triplet(value: &Value) -> Result<(u16, u8, u8), String> {
    Ok((
        value.get("loc").as_i64().ok_or("triplet.loc")? as u16,
        value.get("shape").as_i64().ok_or("triplet.shape")? as u8,
        value.get("angle").as_i64().ok_or("triplet.angle")? as u8,
    ))
}

/// Loc ids off a `door_defs` request. Empty batch is valid (no-op reply).
pub fn parse_door_defs_request(request: &Value) -> Result<Vec<u16>, String> {
    request
        .get("locs")
        .as_arr()
        .ok_or("locs")?
        .iter()
        .map(|loc| loc.as_i64().ok_or("loc id".to_string()).map(|loc| loc as u16))
        .collect()
}

/// Reply shape: `{ok, version, defs}`. Absent op encodes null (never 0:
/// 1-based op index), absent toll never auto-pays.
pub fn encode_door_defs_reply(defs: &[DoorDef], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        (
            "defs",
            Value::Arr(
                defs.iter()
                    .map(|def| {
                        object(vec![
                            ("loc", Value::Int(i64::from(def.loc))),
                            (
                                "open_op",
                                def.open_op.map_or(Value::Null, |op| Value::Int(i64::from(op))),
                            ),
                            ("toll", Value::Bool(def.toll)),
                            ("blockwalk", Value::Bool(def.blockwalk)),
                            ("blockrange", Value::Bool(def.blockrange)),
                            ("length", Value::Int(i64::from(def.length))),
                            ("width", Value::Int(i64::from(def.width))),
                            ("active", Value::Int(i64::from(def.active))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn decode_door_defs_reply(reply: &Value) -> Result<Vec<DoorDef>, String> {
    reply
        .get("defs")
        .as_arr()
        .ok_or("defs")?
        .iter()
        .map(|def| {
            Ok(DoorDef {
                loc: def.get("loc").as_i64().ok_or("def.loc")? as u16,
                open_op: def.get("open_op").as_i64().map(|op| op as u8),
                toll: def.get("toll").as_bool().unwrap_or(false),
                blockwalk: def.get("blockwalk").as_bool().unwrap_or(false),
                blockrange: def.get("blockrange").as_bool().unwrap_or(false),
                length: def.get("length").as_i64().unwrap_or(0) as i32,
                width: def.get("width").as_i64().unwrap_or(0) as i32,
                active: def.get("active").as_i64().unwrap_or(0) as i32,
            })
        })
        .collect()
}

/// Tiles off a `tiles_flags` request, in order. Empty batch is valid.
pub fn parse_tiles_flags_request(request: &Value) -> Result<Vec<Tile>, String> {
    request
        .get("tiles")
        .as_arr()
        .ok_or("tiles")?
        .iter()
        .map(|tile| {
            let pair = tile.as_arr().ok_or("tile pair")?;
            Ok(Tile {
                x: pair.first().and_then(Value::as_i64).ok_or("tile x")? as i32,
                z: pair.get(1).and_then(Value::as_i64).ok_or("tile z")? as i32,
                level: pair.get(2).and_then(Value::as_i64).ok_or("tile level")? as i32,
            })
        })
        .collect()
}

/// Reply shape: `{ok, version, tiles}` with one entry per requested tile,
/// in request order. Absent door/loc encode null.
pub fn encode_tiles_flags_reply(rows: &[TileFlags], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        (
            "tiles",
            Value::Arr(
                rows.iter()
                    .map(|row| {
                        object(vec![
                            ("tile", tile_value(row.tile)),
                            ("flags", Value::Int(i64::from(row.flags))),
                            (
                                "door",
                                row.door.map_or(Value::Null, triplet_value),
                            ),
                            ("toll", Value::Bool(row.toll)),
                            ("loc", row.loc.map_or(Value::Null, triplet_value)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn decode_tiles_flags_reply(reply: &Value) -> Result<Vec<TileFlags>, String> {
    reply
        .get("tiles")
        .as_arr()
        .ok_or("tiles")?
        .iter()
        .map(|row| {
            Ok(TileFlags {
                tile: parse_tile(row.get("tile"))?,
                flags: row.get("flags").as_i64().ok_or("tile.flags")? as u32,
                door: if row.get("door").is_null() {
                    None
                } else {
                    Some(parse_triplet(row.get("door"))?)
                },
                toll: row.get("toll").as_bool().unwrap_or(false),
                loc: if row.get("loc").is_null() {
                    None
                } else {
                    Some(parse_triplet(row.get("loc"))?)
                },
            })
        })
        .collect()
}

/// Static loc near a center tile: id plus the shape/angle the footprint
/// holder needs once live names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticLoc {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub loc: u16,
    pub shape: u8,
    pub angle: u8,
}

/// Static locs within a Manhattan radius on one level, nearest first.
/// Diamond scan through the public tile read (same one-lock-per-tile
/// discipline as `tiles_flags`): no worker-side `static_locs` scan, and
/// name resolution stays worker-side.
pub fn static_locs_near(snapshot: &Snapshot, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Vec<StaticLoc> {
    let radius = radius.max(0);
    let mut out = Vec::new();
    for scan_x in tile_x - radius..=tile_x + radius {
        let rest = radius - (scan_x - tile_x).abs();
        for scan_z in tile_z - rest..=tile_z + rest {
            if let Some((loc, shape, angle)) = snapshot.world.loc_at(scan_x, scan_z, level) {
                out.push(StaticLoc { x: scan_x, z: scan_z, level, loc, shape, angle });
            }
        }
    }
    out.sort_by_key(|entry| ((entry.x - tile_x).abs() + (entry.z - tile_z).abs(), entry.x, entry.z, entry.loc));
    out
}

/// One F2P verdict per tile, in request order. Level ignored like the
/// engine check; unset bounds read as inside.
pub fn in_f2p_batch(snapshot: &Snapshot, tiles: &[Tile]) -> Vec<bool> {
    tiles.iter().map(|tile| snapshot.world.in_f2p(tile.x, tile.z)).collect()
}

/// Nearest toll-gate tile on this level within a Manhattan radius, if
/// any. Diamond scan through the public toll read; its x is the fence
/// line, the tile itself the approach point.
pub fn toll_fence(snapshot: &Snapshot, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Option<(i32, i32, i32)> {
    let radius = radius.max(0);
    let mut best: Option<((i32, i32, i32), i32)> = None;
    for scan_x in tile_x - radius..=tile_x + radius {
        let rest = radius - (scan_x - tile_x).abs();
        for scan_z in tile_z - rest..=tile_z + rest {
            if snapshot.world.is_toll(scan_x, scan_z, level) {
                let dist = (scan_x - tile_x).abs() + (scan_z - tile_z).abs();
                if best.is_none_or(|(_, known)| dist < known) {
                    best = Some(((scan_x, scan_z, level), dist));
                }
            }
        }
    }
    best.map(|(tile, _)| tile)
}

/// Every tag a plan may require, sorted and distinct, for the worker to
/// satisfy from live state. Versioned by the reply envelope.
pub fn wants(snapshot: &Snapshot) -> Vec<String> {
    let mut tags: Vec<String> = snapshot
        .tables
        .requirements
        .iter()
        .flat_map(|entry| entry.requires.iter().cloned())
        .chain(snapshot.tables.customs.iter().flat_map(|edge| edge.requires.iter().cloned()))
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn static_loc_value(entry: &StaticLoc) -> Value {
    object(vec![
        ("x", Value::Int(i64::from(entry.x))),
        ("z", Value::Int(i64::from(entry.z))),
        ("level", Value::Int(i64::from(entry.level))),
        ("loc", Value::Int(i64::from(entry.loc))),
        ("shape", Value::Int(i64::from(entry.shape))),
        ("angle", Value::Int(i64::from(entry.angle))),
    ])
}

fn parse_static_loc(value: &Value) -> Result<StaticLoc, String> {
    Ok(StaticLoc {
        x: value.get("x").as_i64().ok_or("loc.x")? as i32,
        z: value.get("z").as_i64().ok_or("loc.z")? as i32,
        level: value.get("level").as_i64().ok_or("loc.level")? as i32,
        loc: value.get("loc").as_i64().ok_or("loc.loc")? as u16,
        shape: value.get("shape").as_i64().unwrap_or(0) as u8,
        angle: value.get("angle").as_i64().unwrap_or(0) as u8,
    })
}

/// Center + radius off a center/radius request.
pub fn parse_center_radius(request: &Value) -> Result<(i32, i32, i32, i32), String> {
    Ok((
        request.get("x").as_i64().ok_or("x")? as i32,
        request.get("z").as_i64().ok_or("z")? as i32,
        request.get("level").as_i64().ok_or("level")? as i32,
        request.get("radius").as_i64().ok_or("radius")? as i32,
    ))
}

/// Center + radius off a `static_locs_near` request.
pub fn parse_static_locs_near_request(request: &Value) -> Result<(i32, i32, i32, i32), String> {
    parse_center_radius(request)
}

/// Reply shape: `{ok, version, locs}`, nearest first.
pub fn encode_static_locs_near_reply(locs: &[StaticLoc], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        ("locs", Value::Arr(locs.iter().map(static_loc_value).collect())),
    ])
}

pub fn decode_static_locs_near_reply(reply: &Value) -> Result<Vec<StaticLoc>, String> {
    reply.get("locs").as_arr().ok_or("locs")?.iter().map(parse_static_loc).collect()
}

/// Reply shape: `{ok, version, inside}`, one bool per requested tile,
/// in request order.
pub fn encode_in_f2p_reply(inside: &[bool], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        ("inside", Value::Arr(inside.iter().map(|flag| Value::Bool(*flag)).collect())),
    ])
}

pub fn decode_in_f2p_reply(reply: &Value) -> Result<Vec<bool>, String> {
    reply
        .get("inside")
        .as_arr()
        .ok_or("inside")?
        .iter()
        .map(|flag| flag.as_bool().ok_or("inside flag".to_string()))
        .collect()
}

/// Center + radius off a `toll_fence` request.
pub fn parse_toll_fence_request(request: &Value) -> Result<(i32, i32, i32, i32), String> {
    parse_center_radius(request)
}

/// Reply shape: `{ok, version, gate}`, the nearest toll-gate tile or
/// null when no fence stands within the radius.
pub fn encode_toll_fence_reply(gate: Option<(i32, i32, i32)>, version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        (
            "gate",
            gate.map_or(Value::Null, |(x, z, level)| {
                object(vec![
                    ("x", Value::Int(i64::from(x))),
                    ("z", Value::Int(i64::from(z))),
                    ("level", Value::Int(i64::from(level))),
                ])
            }),
        ),
    ])
}

pub fn decode_toll_fence_reply(reply: &Value) -> Result<Option<(i32, i32, i32)>, String> {
    let gate = reply.get("gate");
    if gate.is_null() {
        return Ok(None);
    }
    Ok(Some((
        gate.get("x").as_i64().ok_or("gate.x")? as i32,
        gate.get("z").as_i64().ok_or("gate.z")? as i32,
        gate.get("level").as_i64().ok_or("gate.level")? as i32,
    )))
}

/// Reply shape: `{ok, version, wants}`, sorted and distinct.
pub fn encode_wants_reply(tags: &[String], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        ("wants", Value::Arr(tags.iter().map(|tag| str_value(tag)).collect())),
    ])
}

pub fn decode_wants_reply(reply: &Value) -> Result<Vec<String>, String> {
    reply
        .get("wants")
        .as_arr()
        .ok_or("wants")?
        .iter()
        .map(|tag| tag.as_str().ok_or("wants tag".to_string()).map(str::to_string))
        .collect()
}

/// One live cell of a dossier window: tile plus the live loc on it.
/// `removed` marks a live-gone loc; absent live state never ships.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffCell {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub loc: Option<(u16, u8, u8)>,
    pub removed: bool,
}

/// One mismatch between snapshot template and worker live window: `x`,
/// `z`, `level` keys classify B in `replay-nav`, values carry triplets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffEntry {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub static_loc: Option<(u16, u8, u8)>,
    pub live_loc: Option<(u16, u8, u8)>,
}

/// Static against live, per cell, in request order: agreeing tiles ship
/// nothing, unstreamed tiles never fake a verdict (worker omits them).
pub fn diff_cells(snapshot: &Snapshot, cells: &[DiffCell]) -> Vec<DiffEntry> {
    cells
        .iter()
        .filter_map(|cell| {
            let live = if cell.removed { None } else { cell.loc };
            let named = snapshot.world.loc_at(cell.x, cell.z, cell.level);
            (named != live).then(|| DiffEntry { x: cell.x, z: cell.z, level: cell.level, static_loc: named, live_loc: live })
        })
        .collect()
}

fn live_triplet_value(triplet: Option<(u16, u8, u8)>) -> Value {
    triplet.map_or(Value::Null, triplet_value)
}

fn parse_live_triplet(value: &Value) -> Result<Option<(u16, u8, u8)>, String> {
    if value.is_null() {
        return Ok(None);
    }
    parse_triplet(value).map(Some)
}

fn diff_cell_value(cell: &DiffCell) -> Value {
    object(vec![
        ("x", Value::Int(i64::from(cell.x))),
        ("z", Value::Int(i64::from(cell.z))),
        ("level", Value::Int(i64::from(cell.level))),
        ("loc", live_triplet_value(cell.loc)),
        ("removed", Value::Bool(cell.removed)),
    ])
}

fn parse_diff_cell(value: &Value) -> Result<DiffCell, String> {
    Ok(DiffCell {
        x: value.get("x").as_i64().ok_or("cell.x")? as i32,
        z: value.get("z").as_i64().ok_or("cell.z")? as i32,
        level: value.get("level").as_i64().ok_or("cell.level")? as i32,
        loc: parse_live_triplet(value.get("loc"))?,
        removed: value.get("removed").as_bool().unwrap_or(false),
    })
}

/// Cells off a `diff` request: the worker live window. Empty is valid.
pub fn parse_diff_request(request: &Value) -> Result<Vec<DiffCell>, String> {
    request.get("cells").as_arr().ok_or("cells")?.iter().map(parse_diff_cell).collect()
}

/// Cells for a `diff` request body.
pub fn encode_diff_cells(cells: &[DiffCell]) -> Value {
    Value::Arr(cells.iter().map(diff_cell_value).collect())
}

/// Reply shape: `{ok, version, diff}`, one entry per mismatch.
pub fn encode_diff_reply(entries: &[DiffEntry], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        (
            "diff",
            Value::Arr(
                entries
                    .iter()
                    .map(|entry| {
                        object(vec![
                            ("x", Value::Int(i64::from(entry.x))),
                            ("z", Value::Int(i64::from(entry.z))),
                            ("level", Value::Int(i64::from(entry.level))),
                            ("static", live_triplet_value(entry.static_loc)),
                            ("live", live_triplet_value(entry.live_loc)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

pub fn decode_diff_reply(reply: &Value) -> Result<Vec<DiffEntry>, String> {
    reply
        .get("diff")
        .as_arr()
        .ok_or("diff")?
        .iter()
        .map(|entry| {
            Ok(DiffEntry {
                x: entry.get("x").as_i64().ok_or("diff.x")? as i32,
                z: entry.get("z").as_i64().ok_or("diff.z")? as i32,
                level: entry.get("level").as_i64().ok_or("diff.level")? as i32,
                static_loc: parse_live_triplet(entry.get("static"))?,
                live_loc: parse_live_triplet(entry.get("live"))?,
            })
        })
        .collect()
}

/// Static half of a `debug` answer: flags here and at dest, template
/// locs within two tiles of dest, and their defs for the open-op label.
pub fn debug_static(snapshot: &Snapshot, from: &Tile, dest: &Tile) -> (u32, u32, Vec<StaticLoc>, Vec<DoorDef>) {
    let (flags_here, flags_dest) = (snapshot.world.get(from.x, from.z, from.level), snapshot.world.get(dest.x, dest.z, dest.level));
    let locs = static_locs_near(snapshot, dest.x, dest.z, dest.level, 2);
    let mut ids: Vec<u16> = locs.iter().map(|entry| entry.loc).collect();
    ids.sort();
    ids.dedup();
    let defs = door_defs(snapshot, &ids);
    (flags_here, flags_dest, locs, defs)
}

/// From, dest and options off a `debug` request.
pub fn parse_debug_request(request: &Value) -> Result<(Tile, Tile, NavOptions), String> {
    Ok((parse_tile(request.get("from"))?, parse_tile(request.get("dest"))?, crate::service::decode_options(request.get("options"))?))
}

/// Reply shape: `{ok, version, flags_here, flags_dest, locs, defs}`
/// plus exactly one of `plan` / `noroute` (plan codec, old tolerated).
pub fn encode_debug_reply(flags_here: u32, flags_dest: u32, locs: &[StaticLoc], defs: &[DoorDef], outcome: &Result<Plan, NoRoute>, version: &str) -> Value {
    let mut fields = vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        ("flags_here", Value::Int(i64::from(flags_here))),
        ("flags_dest", Value::Int(i64::from(flags_dest))),
        ("locs", encode_static_locs_near_reply(locs, version).get("locs").clone()),
        ("defs", encode_door_defs_reply(defs, version).get("defs").clone()),
    ];
    match outcome {
        Ok(plan) => fields.push(("plan", crate::service::encode_plan(plan))),
        Err(no) => fields.push(("noroute", crate::service::encode_noroute(no))),
    }
    object(fields)
}

/// Decoded `debug` answer: static half plus plan or NoRoute diagnosis.
#[derive(Debug)]
pub struct DebugAnswer {
    pub flags_here: u32,
    pub flags_dest: u32,
    pub locs: Vec<StaticLoc>,
    pub defs: Vec<DoorDef>,
    pub outcome: Result<Plan, NoRoute>,
}

pub fn decode_debug_reply(reply: &Value) -> Result<DebugAnswer, String> {
    let locs = decode_static_locs_near_reply(&object(vec![("locs", reply.get("locs").clone())]))?;
    let defs = decode_door_defs_reply(&object(vec![("defs", reply.get("defs").clone())]))?;
    let outcome = if reply.get("plan").is_null() {
        Err(crate::service::decode_noroute(reply.get("noroute"))?)
    } else {
        Ok(crate::service::decode_plan(reply.get("plan"))?)
    };
    Ok(DebugAnswer {
        flags_here: reply.get("flags_here").as_i64().ok_or("flags_here")? as u32,
        flags_dest: reply.get("flags_dest").as_i64().ok_or("flags_dest")? as u32,
        locs,
        defs,
        outcome,
    })
}

/// One transition row for admin tools: every field the live probes and
/// checks read. The file `kind` column has no typed equivalent, so
/// bulk rows carry none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionView {
    pub name: String,
    pub from: Tile,
    pub to: Tile,
    pub loc: u16,
    pub climb_loc: u16,
    pub climb_op: u8,
    pub footprint: Vec<(i32, i32)>,
    pub delta: Option<(i32, i32, i32)>,
    pub open_to: Option<u16>,
    pub disabled: bool,
}

impl TransitionView {
    fn of(row: &Transition) -> Self {
        TransitionView {
            name: row.name.clone(),
            from: row.from,
            to: row.to,
            loc: row.loc,
            climb_loc: row.climb_loc,
            climb_op: row.climb_op,
            footprint: row.footprint.clone(),
            delta: row.delta,
            open_to: row.open_to,
            disabled: row.disabled,
        }
    }
}

/// Every transition row, in table order (disabled included).
pub fn transitions_list(snapshot: &Snapshot) -> Vec<TransitionView> {
    snapshot.tables.transitions.iter().map(TransitionView::of).collect()
}

fn transition_view_value(view: &TransitionView) -> Value {
    object(vec![
        ("name", str_value(&view.name)),
        ("from", tile_value(view.from)),
        ("to", tile_value(view.to)),
        ("loc", Value::Int(i64::from(view.loc))),
        ("climb_loc", Value::Int(i64::from(view.climb_loc))),
        ("climb_op", Value::Int(i64::from(view.climb_op))),
        ("footprint", Value::Arr(view.footprint.iter().map(|(x, z)| Value::Arr(vec![Value::Int(i64::from(*x)), Value::Int(i64::from(*z))])).collect())),
        ("delta", view.delta.map_or(Value::Null, |(level, x, z)| tile_value(Tile { level, x, z }))),
        ("open_to", view.open_to.map_or(Value::Null, |id| Value::Int(i64::from(id)))),
        ("disabled", Value::Bool(view.disabled)),
    ])
}

fn parse_transition_view(value: &Value) -> Result<TransitionView, String> {
    let footprint = value
        .get("footprint")
        .as_arr()
        .ok_or("footprint")?
        .iter()
        .map(|tile| {
            let pair = tile.as_arr().ok_or("footprint pair")?;
            Ok((pair.first().and_then(Value::as_i64).ok_or("footprint x")? as i32, pair.get(1).and_then(Value::as_i64).ok_or("footprint z")? as i32))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let delta = if value.get("delta").is_null() {
        None
    } else {
        let tile = parse_tile(value.get("delta"))?;
        Some((tile.level, tile.x, tile.z))
    };
    Ok(TransitionView {
        name: value.get("name").as_str().ok_or("name")?.to_string(),
        from: parse_tile(value.get("from"))?,
        to: parse_tile(value.get("to"))?,
        loc: value.get("loc").as_i64().ok_or("loc")? as u16,
        climb_loc: value.get("climb_loc").as_i64().ok_or("climb_loc")? as u16,
        climb_op: value.get("climb_op").as_i64().unwrap_or(1) as u8,
        footprint,
        delta,
        open_to: if value.get("open_to").is_null() { None } else { Some(value.get("open_to").as_i64().ok_or("open_to")? as u16) },
        disabled: value.get("disabled").as_bool().unwrap_or(false),
    })
}

/// Reply shape: `{ok, version, transitions}`, in table order.
pub fn encode_transitions_list_reply(rows: &[TransitionView], version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("version", str_value(version)),
        ("transitions", Value::Arr(rows.iter().map(transition_view_value).collect())),
    ])
}

pub fn decode_transitions_list_reply(reply: &Value) -> Result<Vec<TransitionView>, String> {
    reply.get("transitions").as_arr().ok_or("transitions")?.iter().map(parse_transition_view).collect()
}

/// One probe target: the row plus the stand the prober rides from and
/// the tile it must land on. No reach-valid stand reads `None`: the
/// worker logs the skip like the local computation did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeTarget {
    pub view: TransitionView,
    pub stand: Option<Tile>,
    pub expect: Option<Tile>,
}

/// Same filter as the worker prober: name substring, exact loc id,
/// table-coordinate substring (covers zone numbers like 9858).
fn transition_matches(row: &Transition, filter: Option<&str>) -> bool {
    let Some(raw) = filter else { return true; };
    let needle = raw.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    if needle.parse::<u16>().is_ok_and(|num| num == row.loc || num == row.climb_loc) {
        return true;
    }
    if row.name.to_lowercase().contains(&needle) {
        return true;
    }
    format!("{} {} {} {} {} {}", row.from.level, row.from.x, row.from.z, row.to.level, row.to.x, row.to.z).contains(&needle)
}

/// Stands and expects for the filtered rows, prober options (paid
/// doors open, no tags held). One verdict covers the whole row.
pub fn probe_targets(snapshot: &Snapshot, filter: Option<&str>) -> Vec<ProbeTarget> {
    let options = NavOptions { allow_paid: true, f2p_only: true, satisfied: HashSet::new(), excluded_edges: HashSet::new(), danger: Vec::new() };
    let mut out = Vec::new();
    for row in &snapshot.tables.transitions {
        if row.disabled || !transition_matches(row, filter) {
            continue;
        }        let stand = crate::plan::ride_stands(&snapshot.world, &snapshot.tables, &options, row).into_iter().next();
        let expect = stand.map(|found| row.delta.map_or(row.to, |(dl, dx, dz)| Tile { x: found.x + dx, z: found.z + dz, level: found.level + dl }));
        out.push(ProbeTarget { view: TransitionView::of(row), stand, expect });
    }
    out
}

/// Reply shape: `{ok, version, targets}`, table order; the row half
/// reuses the list codec, `stand`/`expect` are null on skipped rows.
pub fn encode_probe_targets_reply(targets: &[ProbeTarget], version: &str) -> Value {
    let targets = targets
        .iter()
        .map(|target| {
            let mut entry = transition_view_value(&target.view);
            if let Value::Obj(ref mut fields) = entry {
                fields.insert("stand".to_string(), target.stand.map_or(Value::Null, tile_value));
                fields.insert("expect".to_string(), target.expect.map_or(Value::Null, tile_value));
            }
            entry
        })
        .collect();
    object(vec![("ok", Value::Bool(true)), ("version", str_value(version)), ("targets", Value::Arr(targets))])
}

pub fn decode_probe_targets_reply(reply: &Value) -> Result<Vec<ProbeTarget>, String> {
    reply
        .get("targets")
        .as_arr()
        .ok_or("targets")?
        .iter()
        .map(|target| {
            let view = parse_transition_view(target)?;
            let stand = if target.get("stand").is_null() { None } else { Some(parse_tile(target.get("stand"))?) };
            let expect = if target.get("expect").is_null() { None } else { Some(parse_tile(target.get("expect"))?) };
            Ok(ProbeTarget { view, stand, expect })
        })
        .collect()
}

/// Static doors within a Manhattan radius, nearest first: template
/// locs advertising an Open op. Names and live state resolve worker-side.
pub fn doors_near(snapshot: &Snapshot, tile_x: i32, tile_z: i32, level: i32, radius: i32) -> Vec<StaticLoc> {
    static_locs_near(snapshot, tile_x, tile_z, level, radius)
        .into_iter()
        .filter(|entry| snapshot.world.loc_def(usize::from(entry.loc)).is_some_and(|def| def.open_op.is_some()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{self, NavOptions, Segment};

    fn setup() -> (std::sync::Arc<crate::static_world::StaticWorld>, std::sync::Arc<crate::tables::Tables>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let zip = root.join("lostcity/engine/data/pack/.cache/maps-server.zip");
        let gametypes = std::fs::read_to_string(root.join("LifeScape/gamedata/data/gametypes.json")).expect("gametypes");
        let overrides = std::fs::read_to_string(root.join("LifeScape/gamedata/data/nav_overrides.json")).expect("overrides");
        let defs = crate::loctypes::parse_loctypes(&gametypes, &overrides).expect("loctypes");
        let world = crate::static_world::StaticWorld::open(&zip, defs).expect("static world");
        let zones = crate::f2p::load_zones(&root.join("lostcity/content/maps/free2play.csv")).expect("f2p");
        world.set_f2p(zones);
        let tables = crate::tables::Tables::load(&root.join("LifeScape/gamedata/data")).expect("tables");
        world.set_sky_floors(tables.sky_floors.clone());
        (world, std::sync::Arc::new(tables))
    }

    fn door_leg_plan(
        world: &crate::static_world::StaticWorld,
        tables: &crate::tables::Tables,
    ) -> plan::Plan {
        let from = Tile { x: 3228, z: 3214, level: 0 };
        let dest = Tile { x: 3224, z: 3214, level: 0 };
        let mut plan = None;
        for _ in 0..5 {
            if let Ok(found) = plan::plan(world, tables, from, dest, &NavOptions::default()) {
                plan = Some(found);
                break;
            }
        }
        plan.expect("door leg plans")
    }

    #[test]
    fn codec_round_trip_is_exact() {
        let def = DoorDef { loc: 1516, open_op: Some(1), toll: false, blockwalk: true, blockrange: true, length: 1, width: 1, active: 1 };
        let reply = encode_door_defs_reply(std::slice::from_ref(&def), "v1");
        assert_eq!(decode_door_defs_reply(&reply).expect("defs decode"), vec![def]);
        let row = TileFlags {
            tile: Tile { x: 3226, z: 3223, level: 0 },
            flags: 0x240100,
            door: Some((1530, 0, 2)),
            toll: false,
            loc: Some((1530, 0, 2)),
        };
        let reply = encode_tiles_flags_reply(std::slice::from_ref(&row), "v1");
        assert_eq!(decode_tiles_flags_reply(&reply).expect("tiles decode"), vec![row]);
        // Unknown loc id: shut, never-paid, zero footprint — never an error.
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world, tables, "v".to_string());
        let reply = encode_door_defs_reply(&door_defs(&snapshot, &[65000]), "v");
        let defs = decode_door_defs_reply(&reply).expect("unknown decodes");
        assert_eq!(defs[0].open_op, None, "unknown loc never clicks blind");
        assert!(!defs[0].toll, "unknown loc never auto-pays");
    }

    #[test]
    fn queries_match_the_planner_inputs() {
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world.clone(), tables.clone(), "q".to_string());
        let plan = door_leg_plan(&world, &tables);
        let door = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                Segment::OpenDoor { door, .. } => Some(door.clone()),
                _ => None,
            })
            .expect("door leg swings a door");
        // The def carries exactly what the plan segment rode on.
        let defs = door_defs(&snapshot, &[door.loc]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].open_op, door.open_op, "def matches the planned click");
        assert_eq!(defs[0].toll, door.toll, "def matches the planned toll");
        assert!(defs[0].open_op.is_some(), "the door leg carries its open op");
        // Tile flags name the same door on its tile, flags equal a direct read.
        let rows = tiles_flags(&snapshot, &[Tile { x: door.x, z: door.z, level: door.level }]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].flags, world.get(door.x, door.z, door.level), "flags equal the world read");
        assert!(rows[0].door.is_some(), "door tile names its door");
        assert_eq!(rows[0].toll, door.toll, "toll matches the planned flag");
    }

    #[test]
    fn l4_codecs_round_trip_exact() {
        let loc = StaticLoc { x: 3226, z: 3214, level: 0, loc: 1530, shape: 0, angle: 2 };
        let reply = encode_static_locs_near_reply(std::slice::from_ref(&loc), "v1");
        assert_eq!(decode_static_locs_near_reply(&reply).expect("locs decode"), vec![loc]);
        let reply = encode_static_locs_near_reply(&[], "v1");
        assert!(decode_static_locs_near_reply(&reply).expect("empty decodes").is_empty());
        let reply = encode_in_f2p_reply(&[true, false], "v1");
        assert_eq!(decode_in_f2p_reply(&reply).expect("f2p decodes"), vec![true, false]);
        let reply = encode_toll_fence_reply(Some((3268, 3227, 0)), "v1");
        assert_eq!(decode_toll_fence_reply(&reply).expect("gate decodes"), Some((3268, 3227, 0)));
        let reply = encode_toll_fence_reply(None, "v1");
        assert_eq!(decode_toll_fence_reply(&reply).expect("null decodes"), None);
        let tags = vec!["coins:10".to_string(), "qp:32".to_string()];
        let reply = encode_wants_reply(&tags, "v1");
        assert_eq!(decode_wants_reply(&reply).expect("wants decode"), tags);
        // Old loc bodies without shape/angle read as ground-floor square.
        let old = object(vec![
            ("x", Value::Int(1)),
            ("z", Value::Int(2)),
            ("level", Value::Int(0)),
            ("loc", Value::Int(7)),
        ]);
        let reply = object(vec![
            ("ok", Value::Bool(true)),
            ("version", str_value("v1")),
            ("locs", Value::Arr(vec![old])),
        ]);
        let locs = decode_static_locs_near_reply(&reply).expect("old body decodes");
        assert_eq!((locs[0].shape, locs[0].angle), (0, 0), "absent shape reads zero");
    }

    #[test]
    fn static_locs_near_names_the_planned_door() {
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world.clone(), tables.clone(), "near".to_string());
        let plan = door_leg_plan(&world, &tables);
        let door = plan
            .segments
            .iter()
            .find_map(|segment| match segment {
                Segment::OpenDoor { door, .. } => Some(door.clone()),
                _ => None,
            })
            .expect("door leg swings a door");
        let found = static_locs_near(&snapshot, door.x, door.z, door.level, 2);
        assert!(found.iter().any(|entry| entry.loc == door.loc), "the door tile names its loc");
        assert!(found.windows(2).all(|pair| {
            let first = (pair[0].x - door.x).abs() + (pair[0].z - door.z).abs();
            let second = (pair[1].x - door.x).abs() + (pair[1].z - door.z).abs();
            first <= second
        }), "nearest first");
        assert!(found.iter().all(|entry| entry.level == door.level), "level filters");
    }

    #[test]
    fn in_f2p_matches_the_direct_read() {
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world.clone(), tables, "f2p".to_string());
        let tiles = vec![
            Tile { x: 3091, z: 3242, level: 0 },
            Tile { x: 2862, z: 3385, level: 0 },
            Tile { x: 3228, z: 3214, level: 0 },
        ];
        let inside = in_f2p_batch(&snapshot, &tiles);
        assert_eq!(inside.len(), tiles.len(), "one verdict per tile, in order");
        for (tile, flag) in tiles.iter().zip(inside.iter()) {
            assert_eq!(*flag, world.in_f2p(tile.x, tile.z), "batch equals the direct read");
        }
        assert!(inside[0], "Draynor bank booth is inside F2P");
    }

    #[test]
    fn toll_fence_marks_the_gate_line() {
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world.clone(), tables.clone(), "fence".to_string());
        // The paid corridor crosses a toll gate: fence the leg around it.
        let from = Tile { x: 3265, z: 3227, level: 0 };
        let dest = Tile { x: 3270, z: 3227, level: 0 };
        let mut paid = NavOptions::default();
        paid.allow_paid = true;
        let mut toll_door = None;
        for _ in 0..5 {
            if let Ok(plan) = plan::plan(&world, &tables, from, dest, &paid) {
                toll_door = plan.segments.iter().find_map(|segment| match segment {
                    Segment::OpenDoor { door, .. } if door.toll => Some(door.clone()),
                    _ => None,
                });
                if toll_door.is_some() {
                    break;
                }
            }
        }
        let door = toll_door.expect("the paid corridor crosses a toll gate");
        let gate = toll_fence(&snapshot, door.x, door.z, door.level, 10).expect("a fence stands near its gate");
        assert_eq!((gate.0, gate.2), (door.x, door.level), "the fence line is the gate tile");
        assert!(toll_fence(&snapshot, 3091, 3242, 0, 10).is_none(), "no fence near Draynor bank");
    }

    #[test]
    fn wants_lists_every_planner_tag() {
        let (world, tables) = setup();
        let snapshot = crate::service::Snapshot::new(world, tables, "wants".to_string());
        let tags = wants(&snapshot);
        assert!(tags.contains(&"qp:32".to_string()), "the guild door wants quest points");
        let mut sorted = tags.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(tags, sorted, "sorted and distinct");
    }
}
