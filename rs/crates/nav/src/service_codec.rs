#![forbid(unsafe_code)]
//! Service wire codec (split out of `service.rs`, CPU 2026-11-14): tiles,
//! options, plans and NoRoute verdicts as JSON values. Re-exported by
//! `service` (`service::encode_plan`...), the wire format is unchanged.

use lifescape_json::{object, str_value, Value};

use crate::plan::{Blocker, EdgeId, NavOptions, NoRoute, NoRouteReason, Plan, Segment, Tile, TransitionRef};
use crate::service::edge_key;

// -- JSON codec (options serialized integrally, never normalized) -------------

pub(crate) fn tile_value(tile: Tile) -> Value {
    object(vec![
        ("x", Value::Int(i64::from(tile.x))),
        ("z", Value::Int(i64::from(tile.z))),
        ("level", Value::Int(i64::from(tile.level))),
    ])
}

pub(crate) fn parse_tile(value: &Value) -> Result<Tile, String> {
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
    let zones = value.get("danger").as_arr().map(|rows| rows.as_slice()).unwrap_or_default();
    options.danger.reserve(zones.len());
    for zone in zones {
        let part = |slot: usize| zone.as_arr().and_then(|cells| cells.get(slot)).and_then(|cell| cell.as_i64()).map(|cell| cell as i32).ok_or("danger zone");
        options.danger.push(crate::plan::DangerZone { x: part(0)?, z: part(1)?, level: part(2)?, radius: part(3)? });
    }
    Ok(options)
}

pub(crate) fn ride_value(ride: &TransitionRef) -> Value {
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

