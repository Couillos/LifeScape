#![forbid(unsafe_code)]
//! Service side of the short-hop fast path (`fast_hop.rs`).
//!
//! A `plan` frame whose start and destination sit on one level within
//! `FAST_RADIUS` is answered on the `plan-query` lane that read it: no
//! hand-off to the `plan-wait` pool, no ticket, no results slot, no route
//! cache lookup or insert, no planner setup. The terminal frame is the
//! one `plan_once` reads after a wait (`outcome: ready`), so workers need
//! no change. A `submit` frame takes the same path and answers as an
//! instant ticket. Anything the fast path declines goes the usual way
//! (the geometric pre-check reads two tiles, never the options).

use std::sync::atomic::{AtomicU64, Ordering};

use lifescape_json::{object, str_value, Value};

use crate::fast_hop::FAST_RADIUS;
use crate::plan::{Plan, Tile};
use crate::service::{encode_plan, parse_tile};

/// Fast path counters (`stats` op, `nav-stats`, `cpu-profile nav`).
#[derive(Default)]
pub(crate) struct FastCounters {
    /// Short hops answered here (zero-distance ones included).
    pub hits: AtomicU64,
    /// Zero-distance hops answered (the bot asked for its own tile).
    pub zero: AtomicU64,
    /// Short requests tried and handed to the planner.
    pub declined: AtomicU64,
}

impl FastCounters {
    pub(crate) fn answered(&self, plan: &Plan) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        if plan.segments.is_empty() {
            self.zero.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn read(&self) -> (u64, u64, u64) {
        (self.hits.load(Ordering::Relaxed), self.zero.load(Ordering::Relaxed), self.declined.load(Ordering::Relaxed))
    }
}

/// Start and destination of a short same-level request, else `None`
/// (two tile reads: a long request costs nothing more here).
pub(crate) fn short_pair(request: &Value) -> Option<(Tile, Tile)> {
    let from = parse_tile(request.get("from")).ok()?;
    let dest = parse_tile(request.get("dest")).ok()?;
    (from.level == dest.level && from.chebyshev(dest) <= FAST_RADIUS).then_some((from, dest))
}

/// True when the client pinned another snapshot version: the usual path
/// refuses it stale (the worker learns the version), never the fast one.
pub(crate) fn version_skew(request: &Value, served: &str) -> bool {
    let client = request.get("version").as_str().unwrap_or("");
    !client.is_empty() && !served.is_empty() && client != served
}

/// Terminal `plan` frame, same shape as a waited one (`plan_once`).
pub(crate) fn ready_reply(plan: &Plan, version: &str) -> Value {
    object(vec![
        ("ok", Value::Bool(true)),
        ("outcome", str_value("ready")),
        ("plan", encode_plan(plan)),
        ("version", str_value(version)),
    ])
}
