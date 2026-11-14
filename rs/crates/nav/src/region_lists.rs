#![forbid(unsafe_code)]
//! Region answers a worker can re-serve exactly (CPU 2026-11-14).
//!
//! `toll_fence` and `static_locs_near` are pure functions of (snapshot
//! version, center, level, radius). A list served for a center and a
//! radius R covers every query whose own diamond fits inside it
//! (`|dx| + |dz| + r <= R`, same level): filtering the list gives the
//! exact answer the service would compute, same order, same ties. The
//! worker keeps such lists per version and skips the round trip.
//!
//! Wire: `toll_fence` takes an optional `list_radius` and then answers
//! an extra `gates` array (every toll tile within it, nearest first).
//! Additive both ways: an old daemon ignores the field and answers the
//! single `gate` (the worker then just does not cache), an old worker
//! never sends it.

use lifescape_json::Value;

use crate::service_queries::StaticLoc;

/// Optional request field asking `toll_fence` for the covering list.
pub const LIST_RADIUS_FIELD: &str = "list_radius";
/// Reply field carrying that list.
pub const GATES_FIELD: &str = "gates";

fn manhattan(from: (i32, i32), to: (i32, i32)) -> i32 {
    (from.0 - to.0).abs() + (from.1 - to.1).abs()
}

/// `list_radius` off a `toll_fence` request: `None` when absent.
pub fn list_radius(request: &Value) -> Option<i32> {
    request.get(LIST_RADIUS_FIELD).as_i64().map(|radius| radius as i32)
}

pub fn encode_gates(gates: &[(i32, i32, i32)]) -> Value {
    Value::Arr(
        gates
            .iter()
            .map(|(x, z, level)| Value::Arr(vec![Value::Int(i64::from(*x)), Value::Int(i64::from(*z)), Value::Int(i64::from(*level))]))
            .collect(),
    )
}

/// The `gates` list of a reply, `None` when the daemon did not send one
/// (old daemon: the worker answers from `gate` and caches nothing).
pub fn decode_gates(reply: &Value) -> Option<Vec<(i32, i32, i32)>> {
    let Value::Arr(rows) = reply.get(GATES_FIELD) else {
        return None;
    };
    let mut gates = Vec::with_capacity(rows.len());
    for row in rows {
        let Value::Arr(cells) = row else { return None };
        if cells.len() != 3 {
            return None;
        }
        gates.push((cells[0].as_i64()? as i32, cells[1].as_i64()? as i32, cells[2].as_i64()? as i32));
    }
    Some(gates)
}

/// Every toll tile on `level` within `radius` of the center, served
/// under one version (`StaticWorld::toll_gates_within`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TollList {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub radius: i32,
    pub gates: Vec<(i32, i32, i32)>,
}

impl TollList {
    /// `Some(answer)` when this list covers the query: the nearest gate
    /// within `radius` of (x, z), ties by x then z (the service order).
    /// `None` when the query diamond leaves the list's diamond.
    pub fn answer(&self, x: i32, z: i32, level: i32, radius: i32) -> Option<Option<(i32, i32, i32)>> {
        let radius = radius.max(0);
        if level != self.level || manhattan((x, z), (self.x, self.z)) + radius > self.radius {
            return None;
        }
        Some(
            self.gates
                .iter()
                .filter(|gate| manhattan((x, z), (gate.0, gate.1)) <= radius)
                .min_by_key(|gate| (manhattan((x, z), (gate.0, gate.1)), gate.0, gate.1))
                .copied(),
        )
    }
}

/// Every static loc on `level` within `radius` of the center, as served
/// by `static_locs_near` under one version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocsList {
    pub x: i32,
    pub z: i32,
    pub level: i32,
    pub radius: i32,
    pub locs: Vec<StaticLoc>,
}

impl LocsList {
    /// `Some(answer)` when this list covers the query: the locs within
    /// `radius` of (x, z), nearest first, ties by x, z, loc (the service
    /// sort). `None` when the query diamond leaves the list's diamond.
    pub fn answer(&self, x: i32, z: i32, level: i32, radius: i32) -> Option<Vec<StaticLoc>> {
        let radius = radius.max(0);
        if level != self.level || manhattan((x, z), (self.x, self.z)) + radius > self.radius {
            return None;
        }
        let mut out: Vec<StaticLoc> =
            self.locs.iter().filter(|entry| manhattan((x, z), (entry.x, entry.z)) <= radius).cloned().collect();
        out.sort_by_key(|entry| (manhattan((x, z), (entry.x, entry.z)), entry.x, entry.z, entry.loc));
        Some(out)
    }
}

#[cfg(test)]
#[path = "region_lists_tests.rs"]
mod tests;
