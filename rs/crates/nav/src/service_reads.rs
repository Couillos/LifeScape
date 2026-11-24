#![forbid(unsafe_code)]
//! Grouped reads of the navigation service (lot N6): door defs, tile
//! flags, static locs, free-to-play bits, toll fence, wants, diff, debug,
//! transitions, probe targets, doors near.
//!
//! Eleven methods of `Service` repeated one prelude (version skew
//! refused stale, request parsed, snapshot read, counter bumped) around
//! the pure reads of `service_queries`. Here the prelude is written once
//! (`serve`) and every read is a function of (snapshot, request,
//! counters) returning the reply frame: no socket, no pool thread, no
//! `Service`. The wire frames are the same as before.

use std::sync::atomic::Ordering;

use lifescape_json::{object, str_value, Value};

use crate::plan;
use crate::service::Snapshot;
use crate::service_counters::Counters;
use crate::service_ops::ReadOp;
use crate::service_queries as queries;

fn failure(error: &str, version: &str) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(error)), ("version", str_value(version))])
}

/// The stale-version refusal, when the client pinned another version
/// than the one served (the worker adopts the served version and retries
/// once, never plans local). Counted.
pub(crate) fn stale_refusal(request: &Value, version: &str, counters: &Counters) -> Option<Value> {
    let client_version = request.get("version").as_str().unwrap_or("");
    if client_version.is_empty() || version.is_empty() || client_version == version {
        return None;
    }
    counters.refused_stale.fetch_add(1, Ordering::Relaxed);
    Some(failure(&format!("refused:stale-version client={client_version} server={version}"), version))
}

/// Serve one grouped read off `snapshot`.
pub(crate) fn serve(op: ReadOp, snapshot: &Snapshot, request: &Value, counters: &Counters) -> Value {
    let version = snapshot.version.as_str();
    if let Some(refusal) = stale_refusal(request, version, counters) {
        return refusal;
    }
    let served = |counter: &std::sync::atomic::AtomicU64| {
        counter.fetch_add(1, Ordering::Relaxed);
    };
    match op {
        ReadOp::DoorDefs => match queries::parse_door_defs_request(request) {
            Err(error) => failure(&error, version),
            Ok(locs) => {
                let defs = queries::door_defs(snapshot, &locs);
                served(&counters.door_defs_served);
                queries::encode_door_defs_reply(&defs, version)
            }
        },
        ReadOp::TilesFlags => match queries::parse_tiles_flags_request(request) {
            Err(error) => failure(&error, version),
            Ok(tiles) => {
                let rows = queries::tiles_flags(snapshot, &tiles);
                served(&counters.tiles_flags_served);
                queries::encode_tiles_flags_reply(&rows, version)
            }
        },
        ReadOp::StaticLocsNear => match queries::parse_static_locs_near_request(request) {
            Err(error) => failure(&error, version),
            Ok(center) => {
                let locs = queries::static_locs_near(snapshot, center.0, center.1, center.2, center.3);
                served(&counters.static_locs_served);
                queries::encode_static_locs_near_reply(&locs, version)
            }
        },
        ReadOp::InF2p => match queries::parse_tiles_flags_request(request) {
            Err(error) => failure(&error, version),
            Ok(tiles) => {
                let inside = queries::in_f2p_batch(snapshot, &tiles);
                served(&counters.f2p_served);
                queries::encode_in_f2p_reply(&inside, version)
            }
        },
        ReadOp::TollFence => match queries::parse_toll_fence_request(request) {
            Err(error) => failure(&error, version),
            Ok(center) => {
                served(&counters.toll_fence_served);
                toll_fence(snapshot, request, center, version)
            }
        },
        ReadOp::Wants => {
            let tags = queries::wants(snapshot);
            served(&counters.wants_served);
            queries::encode_wants_reply(&tags, version)
        }
        ReadOp::Diff => match queries::parse_diff_request(request) {
            Err(error) => failure(&error, version),
            Ok(cells) => {
                let entries = queries::diff_cells(snapshot, &cells);
                served(&counters.diff_served);
                queries::encode_diff_reply(&entries, version)
            }
        },
        // The Dijkstra runs inline on the reader thread (admin-only, one
        // caller): a slow exhaustive NoRoute still lands inside the poll
        // guard.
        ReadOp::Debug => match queries::parse_debug_request(request) {
            Err(error) => failure(&error, version),
            Ok((from, dest, options)) => {
                let (flags_here, flags_dest, locs, defs) = queries::debug_static(snapshot, &from, &dest);
                let outcome = plan::plan(&snapshot.world, &snapshot.tables, from, dest, &options);
                served(&counters.debug_served);
                queries::encode_debug_reply(flags_here, flags_dest, &locs, &defs, &outcome, version)
            }
        },
        ReadOp::TransitionsList => {
            let rows = queries::transitions_list(snapshot);
            served(&counters.transitions_list_served);
            queries::encode_transitions_list_reply(&rows, version)
        }
        ReadOp::ProbeTargets => {
            let filter = request.get("filter").as_str().map(str::to_string);
            let targets = queries::probe_targets(snapshot, filter.as_deref());
            served(&counters.probe_targets_served);
            queries::encode_probe_targets_reply(&targets, version)
        }
        ReadOp::DoorsNear => match queries::parse_center_radius(request) {
            Err(error) => failure(&error, version),
            Ok(center) => {
                let doors = queries::doors_near(snapshot, center.0, center.1, center.2, center.3);
                served(&counters.doors_near_served);
                queries::encode_static_locs_near_reply(&doors, version)
            }
        },
    }
}

/// Nearest toll-gate tile within the radius, or null. With a covering
/// list asked for (`region_lists`, additive) the reply also carries every
/// gate of the region: the worker re-serves each query inside it with no
/// round trip. Same region read.
fn toll_fence(snapshot: &Snapshot, request: &Value, center: (i32, i32, i32, i32), version: &str) -> Value {
    let Some(list_radius) = crate::region_lists::list_radius(request) else {
        let gate = queries::toll_fence(snapshot, center.0, center.1, center.2, center.3);
        return queries::encode_toll_fence_reply(gate, version);
    };
    let gates = snapshot.world.toll_gates_within(center.0, center.1, center.2, list_radius.max(center.3));
    let radius = center.3.max(0);
    let gate = gates.iter().find(|gate| (gate.0 - center.0).abs() + (gate.1 - center.1).abs() <= radius).copied();
    let mut reply = queries::encode_toll_fence_reply(gate, version);
    if let Value::Obj(fields) = &mut reply {
        fields.insert(crate::region_lists::GATES_FIELD.to_string(), crate::region_lists::encode_gates(&gates));
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> std::sync::Arc<Snapshot> {
        let (snapshot, _world, _tables) = crate::service_replay_tests::real_snapshot();
        snapshot
    }

    fn request(op: &str, extra: Vec<(&str, Value)>) -> Value {
        let mut fields = vec![("op", str_value(op))];
        fields.extend(extra);
        object(fields)
    }

    #[test]
    fn a_skewed_version_is_refused_and_counted_before_anything_runs() {
        let snapshot = snapshot();
        let counters = Counters::default();
        let skewed = request("transitions_list", vec![("version", str_value("another-version"))]);
        let reply = serve(ReadOp::TransitionsList, &snapshot, &skewed, &counters);
        assert_eq!(reply.get("ok").as_bool(), Some(false));
        assert!(reply.get("error").as_str().unwrap().starts_with("refused:stale-version client=another-version"));
        assert_eq!(reply.get("version").as_str(), Some(snapshot.version.as_str()));
        assert_eq!(counters.refused_stale.load(Ordering::Relaxed), 1);
        assert_eq!(counters.transitions_list_served.load(Ordering::Relaxed), 0, "nothing was served");
        // No version pinned, or the served one: read.
        let plain = serve(ReadOp::TransitionsList, &snapshot, &request("transitions_list", vec![]), &counters);
        assert_eq!(plain.get("ok").as_bool(), Some(true));
        let pinned = request("transitions_list", vec![("version", str_value(&snapshot.version))]);
        assert_eq!(serve(ReadOp::TransitionsList, &snapshot, &pinned, &counters).get("ok").as_bool(), Some(true));
        assert_eq!(counters.transitions_list_served.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_bad_request_answers_a_failure_frame_with_the_version() {
        let snapshot = snapshot();
        let counters = Counters::default();
        let reply = serve(ReadOp::DoorDefs, &snapshot, &request("door_defs", vec![]), &counters);
        assert_eq!(reply.get("ok").as_bool(), Some(false));
        assert!(reply.get("error").as_str().is_some());
        assert_eq!(reply.get("version").as_str(), Some(snapshot.version.as_str()));
        assert_eq!(counters.door_defs_served.load(Ordering::Relaxed), 0, "a refused batch is not served");
    }

    #[test]
    fn reads_count_one_batch_each_and_match_the_pure_functions() {
        let snapshot = snapshot();
        let counters = Counters::default();
        let wants = serve(ReadOp::Wants, &snapshot, &request("wants", vec![]), &counters);
        let tags = queries::decode_wants_reply(&wants).expect("decodes");
        assert_eq!(tags, queries::wants(&snapshot), "the frame carries the pure read");
        assert_eq!(counters.wants_served.load(Ordering::Relaxed), 1);
        let tiles = Value::Arr(vec![Value::Arr(vec![Value::Int(3222), Value::Int(3218), Value::Int(0)])]);
        let flags = serve(ReadOp::TilesFlags, &snapshot, &request("tiles_flags", vec![("tiles", tiles)]), &counters);
        let rows = queries::decode_tiles_flags_reply(&flags).expect("decodes");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].flags, snapshot.world.get(3222, 3218, 0));
        assert_eq!(counters.tiles_flags_served.load(Ordering::Relaxed), 1);
    }
}
