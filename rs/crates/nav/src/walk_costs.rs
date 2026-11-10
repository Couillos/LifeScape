//! Frozen multi-target distance op (PRD 2, N1).
//!
//! Pickers (spot, bank, target choice) need distances to many candidates.
//! One `walk_costs` round-trip serves them all: a single Dijkstra from the
//! bot, stopped when every target is reached, instead of one plan per
//! candidate. Order-preserving: `costs[i]` answers `dests[i]`, `Null`
//! where the target is unplannable. The client batch `Bot::walk_costs`
//! keeps its shape (one call, `Vec<Option<u32>>`); P1.2 wires this op
//! into the service dispatch and the client batching.

#![forbid(unsafe_code)]

use lifescape_json::{object, str_value, Value};

use super::plan::{NavOptions, Tile};
use super::service::{decode_options, encode_options};

/// Wire op name, shared by the client batch and the service dispatch.
pub const WALK_COSTS_OP: &str = "walk_costs";

/// One multi-target distance request: single source, many targets, one
/// option set (same `NavOptions` a plan would use).
#[derive(Debug, Clone)]
pub struct WalkCostsReq {
    pub from: Tile,
    pub dests: Vec<Tile>,
    pub options: NavOptions,
    pub version: String,
}

/// One multi-target answer: `costs[i]` is the tile cost to `dests[i]`,
/// `None` where that target is unplannable. Same `version` echoed back.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkCostsResp {
    pub costs: Vec<Option<u32>>,
    pub version: String,
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

pub fn encode_request(req: &WalkCostsReq) -> Value {
    object(vec![
        ("op", str_value(WALK_COSTS_OP)),
        ("from", tile_value(req.from)),
        (
            "dests",
            Value::Arr(req.dests.iter().map(|tile| tile_value(*tile)).collect()),
        ),
        ("options", encode_options(&req.options)),
        ("version", str_value(&req.version)),
    ])
}

pub fn decode_request(value: &Value) -> Result<WalkCostsReq, String> {
    if value.get("op").as_str().unwrap_or("") != WALK_COSTS_OP {
        return Err(format!("not a {WALK_COSTS_OP} request"));
    }
    let mut dests = Vec::new();
    for tile in value.get("dests").as_arr().ok_or("dests")? {
        dests.push(parse_tile(tile)?);
    }
    Ok(WalkCostsReq {
        from: parse_tile(value.get("from"))?,
        dests,
        options: decode_options(value.get("options"))?,
        version: value.get("version").as_str().unwrap_or("").to_string(),
    })
}

pub fn encode_response(resp: &WalkCostsResp) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("outcome", str_value("costs")),
        (
            "costs",
            Value::Arr(
                resp.costs
                    .iter()
                    .map(|cost| cost.map_or(Value::Null, |price| Value::Int(i64::from(price))))
                    .collect(),
            ),
        ),
        ("version", str_value(&resp.version)),
    ])
}

pub fn decode_response(value: &Value) -> Result<WalkCostsResp, String> {
    if value.get("ok").as_bool() != Some(true) {
        return Err(value.get("error").as_str().unwrap_or("walk_costs refused").to_string());
    }
    let mut costs = Vec::new();
    for cost in value.get("costs").as_arr().ok_or("costs")? {
        if cost.is_null() {
            costs.push(None);
        } else {
            costs.push(Some(cost.as_i64().ok_or("cost")? as u32));
        }
    }
    Ok(WalkCostsResp {
        costs,
        version: value.get("version").as_str().unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_request, decode_response, encode_request, encode_response, WalkCostsReq, WalkCostsResp, WALK_COSTS_OP};
    use crate::plan::{NavOptions, Tile};

    fn req() -> WalkCostsReq {
        WalkCostsReq {
            from: Tile { x: 3200, z: 3200, level: 0 },
            dests: vec![
                Tile { x: 3210, z: 3200, level: 0 },
                Tile { x: 3220, z: 3200, level: 0 },
            ],
            options: NavOptions::default(),
            version: "v1".to_string(),
        }
    }

    /// Requests round-trip: source, targets, options, version.
    #[test]
    fn request_round_trips() {
        let back = decode_request(&encode_request(&req())).expect("request parses back");
        let want = req();
        assert_eq!(back.dests.len(), 2, "every candidate rides along");
        assert_eq!((back.from, back.version), (want.from, want.version));
        assert_eq!(back.dests, want.dests);
        assert_eq!(
            lifescape_json::stringify(&crate::service::encode_options(&back.options)),
            lifescape_json::stringify(&crate::service::encode_options(&want.options)),
            "options survive the trip"
        );
    }

    /// Responses round-trip order-preserving, unreachable as null.
    #[test]
    fn response_keeps_order_and_nulls() {
        let resp = WalkCostsResp { costs: vec![Some(10), None, Some(30)], version: "v1".to_string() };
        let back = decode_response(&encode_response(&resp)).expect("response parses back");
        assert_eq!(back, resp);
        assert_eq!(back.costs[1], None, "unplannable stays unplannable");
    }

    /// Wrong op and refused frames fail loud, never parse as costs.
    #[test]
    fn wrong_frames_fail_loud() {
        assert!(decode_request(&lifescape_json::object(vec![])).is_err());
        let refused = lifescape_json::object(vec![
            ("ok", lifescape_json::Value::Bool(false)),
            ("error", lifescape_json::str_value("refused:queue-full")),
        ]);
        assert!(decode_response(&refused).is_err());
    }

    /// The op name is frozen for both ends.
    #[test]
    fn op_name_is_frozen() {
        assert_eq!(WALK_COSTS_OP, "walk_costs");
        assert_eq!(encode_request(&req()).get("op").as_str(), Some("walk_costs"));
    }
}
