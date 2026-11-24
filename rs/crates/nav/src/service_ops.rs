#![forbid(unsafe_code)]
//! Typed protocol of the navigation service (lot 5.6, 2026-11-24): ONE
//! enum for the operations (`NavOp`), typed replies, and the only place
//! that turns them into JSON frames and back. The socket boundary parses
//! a request once (`NavOp::parse`) and the service works on typed values
//! from there; the in-process caller (`market_eta`) builds the same
//! `PlanRequest` and never touches JSON.
//!
//! The frames are byte-for-byte the ones the service spoke before: same
//! field names, same refusal texts, same version echo. Payload codecs of
//! the plan itself (`service_codec`), of `walk_costs` and of the grouped
//! reads (`service_queries`) stay in their modules; this one frames them.

use lifescape_json::{object, str_value, Value};

use crate::plan::{NavOptions, Tile};
use crate::service::{decode_noroute, decode_options, decode_plan, encode_options, parse_tile, tile_value, Poll, Submit};
use crate::service_plan::PlanOnceError;
use crate::walk_costs::{self, WalkCostsReq};

/// A plan or submit request: who asks, from where to where, under which
/// options, pinned to which data version (`""` = unpinned).
#[derive(Debug, Clone)]
pub struct PlanRequest {
    /// Absent only on a malformed frame: the ticket path refuses it
    /// (`account`), the short-hop lane never read it.
    pub account: Option<String>,
    pub from: Tile,
    pub dest: Tile,
    pub options: NavOptions,
    pub version: String,
}

/// The grouped direct reads (`service_reads`): one op per name, payload
/// parsed by the read itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOp {
    DoorDefs,
    TilesFlags,
    StaticLocsNear,
    InF2p,
    TollFence,
    Wants,
    Diff,
    Debug,
    TransitionsList,
    ProbeTargets,
    DoorsNear,
}

impl ReadOp {
    pub const ALL: [ReadOp; 11] = [
        ReadOp::DoorDefs,
        ReadOp::TilesFlags,
        ReadOp::StaticLocsNear,
        ReadOp::InF2p,
        ReadOp::TollFence,
        ReadOp::Wants,
        ReadOp::Diff,
        ReadOp::Debug,
        ReadOp::TransitionsList,
        ReadOp::ProbeTargets,
        ReadOp::DoorsNear,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ReadOp::DoorDefs => "door_defs",
            ReadOp::TilesFlags => "tiles_flags",
            ReadOp::StaticLocsNear => "static_locs_near",
            ReadOp::InF2p => "in_f2p",
            ReadOp::TollFence => "toll_fence",
            ReadOp::Wants => "wants",
            ReadOp::Diff => "diff",
            ReadOp::Debug => "debug",
            ReadOp::TransitionsList => "transitions_list",
            ReadOp::ProbeTargets => "probe_targets",
            ReadOp::DoorsNear => "doors_near",
        }
    }

    pub fn from_name(name: &str) -> Option<ReadOp> {
        ReadOp::ALL.into_iter().find(|op| op.name() == name)
    }
}

/// One request of the service, parsed once at the socket.
#[derive(Debug)]
pub enum NavOp {
    /// Single-round-trip plan (waits server-side). A request that does
    /// not parse still travels (`Err`): the waiter pool answers the fault.
    Plan { request: Result<PlanRequest, String>, wait_ms: Option<u64> },
    /// Ticket admission (`Err`: the fault text answered with the version).
    Submit(Result<PlanRequest, String>),
    /// Poll a ticket (`None`: the frame carried no id).
    Poll(Option<u64>),
    Cancel(Option<u64>),
    WalkCosts(Result<WalkCostsReq, String>),
    Stats,
    /// A grouped read: the frame stays for the read's own payload codec.
    Read { op: ReadOp, frame: Value },
    /// An op the service does not speak.
    Unknown(String),
}

impl NavOp {
    pub fn parse(frame: Value) -> NavOp {
        let name = frame.get("op").as_str().unwrap_or("").to_string();
        match name.as_str() {
            "plan" => NavOp::Plan {
                request: parse_plan_request(&frame),
                wait_ms: frame.get("wait_ms").as_i64().map(|ms| ms.max(0) as u64),
            },
            "submit" => NavOp::Submit(parse_plan_request(&frame)),
            "poll" => NavOp::Poll(frame.get("id").as_i64().map(|id| id as u64)),
            "cancel" => NavOp::Cancel(frame.get("id").as_i64().map(|id| id as u64)),
            walk_costs::WALK_COSTS_OP => NavOp::WalkCosts(walk_costs::decode_request(&frame)),
            "stats" => NavOp::Stats,
            other => match ReadOp::from_name(other) {
                Some(op) => NavOp::Read { op, frame },
                None => NavOp::Unknown(name),
            },
        }
    }
}

/// The request fields of a `plan` / `submit` frame. A fault names the
/// first missing piece, `account` first (the order the ticket path read
/// them in).
pub fn parse_plan_request(frame: &Value) -> Result<PlanRequest, String> {
    let account = frame.get("account").as_str().map(str::to_string);
    let rest = (|| -> Result<(Tile, Tile, NavOptions), String> {
        Ok((parse_tile(frame.get("from"))?, parse_tile(frame.get("dest"))?, decode_options(frame.get("options"))?))
    })();
    match rest {
        Err(fault) => Err(if account.is_none() { "account".to_string() } else { fault }),
        Ok((from, dest, options)) => Ok(PlanRequest {
            account,
            from,
            dest,
            options,
            version: frame.get("version").as_str().unwrap_or("").to_string(),
        }),
    }
}

fn plan_fields(op: &str, request: &PlanRequest) -> Vec<(&'static str, Value)> {
    vec![
        ("op", str_value(op)),
        ("account", str_value(request.account.as_deref().unwrap_or(""))),
        ("from", tile_value(request.from)),
        ("dest", tile_value(request.dest)),
        ("options", encode_options(&request.options)),
        ("version", str_value(&request.version)),
    ]
}

pub fn encode_submit(request: &PlanRequest) -> Value {
    object(plan_fields("submit", request))
}

pub fn encode_plan_op(request: &PlanRequest, wait_ms: u64) -> Value {
    let mut fields = plan_fields("plan", request);
    fields.push(("wait_ms", Value::Int(wait_ms as i64)));
    object(fields)
}

pub fn encode_poll(id: u64) -> Value {
    object(vec![("op", str_value("poll")), ("id", Value::Int(id as i64))])
}

pub fn encode_cancel(id: u64) -> Value {
    object(vec![("op", str_value("cancel")), ("id", Value::Int(id as i64))])
}

/// `{ok:false, error, version}`: every refusal frame with a version.
pub fn encode_refusal(reason: &str, version: &str) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(reason)), ("version", str_value(version))])
}

/// `{ok:false, error}`: the refusals that never carried a version.
pub fn encode_fault(error: &str) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(error))])
}

// -- submit ---------------------------------------------------------------------

pub fn encode_submit_reply(reply: &Submit) -> Value {
    match reply {
        Submit::Accepted { id, version } => {
            object(vec![("ok", Value::Bool(true)), ("id", Value::Int(*id as i64)), ("version", str_value(version))])
        }
        Submit::Refused { reason, version } => encode_refusal(reason, version),
    }
}

pub fn decode_submit_reply(reply: &Value) -> Result<Submit, String> {
    let version = reply.get("version").as_str().unwrap_or("").to_string();
    if reply.get("ok").as_bool() != Some(true) {
        return Ok(Submit::Refused { reason: reply.get("error").as_str().unwrap_or("refused").to_string(), version });
    }
    Ok(Submit::Accepted { id: reply.get("id").as_i64().ok_or("submit without id")? as u64, version })
}

// -- poll -----------------------------------------------------------------------

/// A ticket read: bodies are the encoded plan / NoRoute the pool stored.
#[derive(Debug)]
pub enum PollReply {
    Pending { version: String },
    Ready { plan: Value, version: String },
    Failed { error: Value, version: String },
    Unknown { version: String },
    NeedsId,
}

pub fn encode_poll_reply(reply: &PollReply) -> Value {
    let framed = |status: &str, body: Option<(&str, &Value)>, version: &str| {
        let mut fields = vec![("ok", Value::Bool(true)), ("status", str_value(status))];
        if let Some((key, value)) = body {
            fields.push((key, value.clone()));
        }
        fields.push(("version", str_value(version)));
        object(fields)
    };
    match reply {
        PollReply::Pending { version } => framed("pending", None, version),
        PollReply::Ready { plan, version } => framed("ready", Some(("plan", plan)), version),
        PollReply::Failed { error, version } => framed("failed", Some(("error", error)), version),
        PollReply::Unknown { version } => encode_refusal("unknown-ticket", version),
        PollReply::NeedsId => encode_fault("poll needs id"),
    }
}

pub fn decode_poll_reply(reply: &Value) -> Result<(Poll, String), String> {
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

pub fn encode_cancel_reply() -> Value {
    object(vec![("ok", Value::Bool(true))])
}

pub fn encode_cancel_needs_id() -> Value {
    encode_fault("cancel needs id")
}

// -- terminal outcome of a `plan` op ------------------------------------------

/// What a `plan` op ends with: the plan, a NoRoute verdict, or a refusal
/// (`error` text: stale version, caps, `plan-timeout`, `unknown-ticket`).
#[derive(Debug)]
pub enum PlanTerminal {
    Ready { plan: Value, version: String },
    Failed { error: Value, version: String },
    Refused { reason: String, version: String },
}

pub fn encode_terminal(terminal: &PlanTerminal) -> Value {
    match terminal {
        PlanTerminal::Ready { plan, version } => object(vec![
            ("ok", Value::Bool(true)),
            ("outcome", str_value("ready")),
            ("plan", plan.clone()),
            ("version", str_value(version)),
        ]),
        PlanTerminal::Failed { error, version } => object(vec![
            ("ok", Value::Bool(true)),
            ("outcome", str_value("failed")),
            ("error", error.clone()),
            ("version", str_value(version)),
        ]),
        PlanTerminal::Refused { reason, version } => encode_refusal(reason, version),
    }
}

/// Shallow read of a terminal frame (the plan body stays encoded).
pub fn parse_terminal(reply: &Value) -> Result<PlanTerminal, String> {
    let version = reply.get("version").as_str().unwrap_or("").to_string();
    if reply.get("ok").as_bool() != Some(true) {
        return Ok(PlanTerminal::Refused { reason: reply.get("error").as_str().unwrap_or("refused").to_string(), version });
    }
    match reply.get("outcome").as_str().unwrap_or("") {
        "ready" => Ok(PlanTerminal::Ready { plan: reply.get("plan").clone(), version }),
        "failed" => Ok(PlanTerminal::Failed { error: reply.get("error").clone(), version }),
        other => Err(format!("bad plan outcome {other:?}")),
    }
}

impl PlanTerminal {
    /// The caller's view: a plan or verdict with the serving version, or
    /// the typed refusal. Decoding the stored body is the only JSON left.
    pub fn into_result(self) -> Result<(Poll, String), PlanOnceError> {
        match self {
            PlanTerminal::Ready { plan, version } => {
                decode_plan(&plan).map(|plan| (Poll::Ready(plan), version)).map_err(PlanOnceError::Transport)
            }
            PlanTerminal::Failed { error, version } => {
                decode_noroute(&error).map(|no| (Poll::Failed(no), version)).map_err(PlanOnceError::Transport)
            }
            PlanTerminal::Refused { reason, version } => Err(PlanOnceError::Refused { reason, served: version }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PlanRequest {
        PlanRequest {
            account: Some("smith".into()),
            from: Tile { x: 3222, z: 3218, level: 0 },
            dest: Tile { x: 3230, z: 3220, level: 0 },
            options: NavOptions {
                allow_paid: false,
                f2p_only: true,
                satisfied: std::collections::HashSet::new(),
                excluded_edges: std::collections::HashSet::new(),
                danger: Vec::new(),
            },
            version: "v7".into(),
        }
    }

    #[test]
    fn plan_and_submit_frames_round_trip_through_the_typed_op() {
        let sent = request();
        match NavOp::parse(encode_submit(&sent)) {
            NavOp::Submit(Ok(got)) => {
                assert_eq!((got.account.as_deref(), got.from, got.dest, got.version.as_str()), (Some("smith"), sent.from, sent.dest, "v7"));
                assert!(got.options.f2p_only && !got.options.allow_paid);
            }
            other => panic!("not a submit: {other:?}"),
        }
        match NavOp::parse(encode_plan_op(&sent, 1234)) {
            NavOp::Plan { request: Ok(got), wait_ms } => {
                assert_eq!(wait_ms, Some(1234));
                assert_eq!(got.dest, sent.dest);
            }
            other => panic!("not a plan: {other:?}"),
        }
        assert!(matches!(NavOp::parse(encode_poll(9)), NavOp::Poll(Some(9))));
        assert!(matches!(NavOp::parse(encode_cancel(4)), NavOp::Cancel(Some(4))));
    }

    #[test]
    fn faults_keep_the_texts_and_the_order_of_the_old_parser() {
        let bare = |fields: Vec<(&str, Value)>| NavOp::parse(object(fields));
        // No account, nothing else: `account` first, as before.
        match bare(vec![("op", str_value("submit"))]) {
            NavOp::Submit(Err(fault)) => assert_eq!(fault, "account"),
            other => panic!("{other:?}"),
        }
        // Account but no tiles: the tile fault.
        match bare(vec![("op", str_value("submit")), ("account", str_value("a"))]) {
            NavOp::Submit(Err(fault)) => assert_eq!(fault, "tile.x"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(bare(vec![("op", str_value("poll"))]), NavOp::Poll(None)));
        assert!(matches!(bare(vec![("op", str_value("cancel"))]), NavOp::Cancel(None)));
        assert!(matches!(bare(vec![("op", str_value("nope"))]), NavOp::Unknown(name) if name == "nope"));
        assert!(matches!(bare(vec![]), NavOp::Unknown(name) if name.is_empty()));
        assert_eq!(encode_poll_reply(&PollReply::NeedsId).get("error").as_str(), Some("poll needs id"));
        assert_eq!(encode_cancel_needs_id().get("error").as_str(), Some("cancel needs id"));
    }

    #[test]
    fn every_read_name_round_trips() {
        for op in ReadOp::ALL {
            assert_eq!(ReadOp::from_name(op.name()), Some(op));
            assert!(matches!(NavOp::parse(object(vec![("op", str_value(op.name()))])), NavOp::Read { op: read, .. } if read == op));
        }
        for name in ["submit", "poll", "cancel", "walk_costs", "stats", "plan", ""] {
            assert_eq!(ReadOp::from_name(name), None, "{name}");
        }
    }

    #[test]
    fn replies_encode_and_decode_the_frames_the_clients_read() {
        let accepted = Submit::Accepted { id: 3, version: "v1".into() };
        match decode_submit_reply(&encode_submit_reply(&accepted)).unwrap() {
            Submit::Accepted { id, version } => assert_eq!((id, version.as_str()), (3, "v1")),
            other => panic!("{other:?}"),
        }
        let refused = Submit::Refused { reason: "refused:queue-full".into(), version: "v1".into() };
        match decode_submit_reply(&encode_submit_reply(&refused)).unwrap() {
            Submit::Refused { reason, version } => assert_eq!((reason.as_str(), version.as_str()), ("refused:queue-full", "v1")),
            other => panic!("{other:?}"),
        }
        let unknown = encode_poll_reply(&PollReply::Unknown { version: "v2".into() });
        assert_eq!(unknown.get("ok").as_bool(), Some(false));
        assert_eq!(unknown.get("error").as_str(), Some("unknown-ticket"));
        assert!(matches!(decode_poll_reply(&unknown), Ok((Poll::Unknown, version)) if version == "v2"));
        let pending = encode_poll_reply(&PollReply::Pending { version: "v2".into() });
        assert!(matches!(decode_poll_reply(&pending), Ok((Poll::Pending, _))));
        let timeout = encode_terminal(&PlanTerminal::Refused { reason: "plan-timeout".into(), version: "v3".into() });
        match parse_terminal(&timeout).unwrap() {
            PlanTerminal::Refused { reason, version } => assert_eq!((reason.as_str(), version.as_str()), ("plan-timeout", "v3")),
            other => panic!("{other:?}"),
        }
        assert!(parse_terminal(&object(vec![("ok", Value::Bool(true)), ("outcome", str_value("odd"))])).is_err());
    }
}
