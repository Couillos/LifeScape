#![forbid(unsafe_code)]
//! The heartbeat frames a worker pushes to the daemon, typed (lot 5.2 of
//! the architecture plan): `snapshot`, `snapshot_light`, `task_report` and
//! `event`, one codec. The JSON stays on the wire; `Frame::decode` reads a
//! line once at the daemon's door, `encode` writes it in the worker, and
//! nothing in between carries a `Value` frame around.
//!
//! Decoding is as tolerant as the readers it replaces: a missing key is
//! `None` (or the default the daemon always applied), a null is a missing
//! key. Encoding writes every key a worker's encoder wrote (a decoded
//! frame re-encodes to the same JSON when it had all of them). Two
//! normalisations, both on frames no worker sends: a `position` that is
//! not three integers is dropped from a light frame, and an event missing
//! `script`, `args`, `message` or `at` gains the empty value of each.

use lifescape_json::{object, str_value, Value};

use crate::job_state::{Phase, Status};

/// A frame the daemon cannot take.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameError {
    /// The line was not a JSON object.
    NotAnObject,
    /// A `type` this build does not know (with the account it named).
    UnknownType { account: String, kind: String },
}

impl FrameError {
    /// The account the frame named (empty when it named none).
    pub fn account(&self) -> &str {
        match self {
            FrameError::NotAnObject => "",
            FrameError::UnknownType { account, .. } => account,
        }
    }
}

/// A full state frame: the envelope (session facts, outside the tree) and
/// the snapshot tree, MOVED out of the line, never copied.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotFrame {
    pub account: String,
    pub phase: Option<Phase>,
    pub status: Option<Status>,
    /// Worker send time.
    pub sent: Option<f64>,
    pub snapshot: Value,
}

/// A light heartbeat: only the keys the daemon merges into the last full
/// state (`merge_into`), plus the session `phase`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LightFrame {
    pub account: String,
    pub script: Option<String>,
    pub status: Option<Status>,
    pub phase: Option<Phase>,
    /// `(level, x, z)`.
    pub position: Option<[i64; 3]>,
    pub ticks: Option<i64>,
    pub zone: Option<String>,
    pub intention: Option<String>,
    pub updated: Option<f64>,
    pub sent: Option<f64>,
    pub npcs_at: Option<Value>,
    pub players_at: Option<Value>,
}

/// A finished task's report (the report itself stays the planner's JSON
/// document: its own reader owns that shape).
#[derive(Clone, Debug, PartialEq)]
pub struct ReportFrame {
    pub account: String,
    pub report: Value,
}

/// A structured event (random event, script error).
#[derive(Clone, Debug, PartialEq)]
pub struct EventFrame {
    pub account: String,
    pub kind: String,
    pub event: String,
    pub resolved: bool,
    pub tick: i64,
    pub script: String,
    pub args: Vec<Value>,
    pub message: String,
    /// Where it happened, as the worker wrote it.
    pub at: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Snapshot(SnapshotFrame),
    Light(LightFrame),
    TaskReport(ReportFrame),
    Event(EventFrame),
}

/// What a frame says of the session (`Job::note_frame`).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionFacts {
    pub phase: Phase,
    pub status: Option<Status>,
    /// Worker send time (`sent`, a light frame's `updated`), if it said.
    pub sent: Option<f64>,
}

fn text(frame: &Value, key: &str) -> Option<String> {
    frame.get(key).as_str().map(str::to_string)
}

fn non_null(frame: &Value, key: &str) -> Option<Value> {
    let value = frame.get(key);
    (!value.is_null()).then(|| value.clone())
}

fn position_of(value: &Value) -> Option<[i64; 3]> {
    let items = value.as_arr()?;
    if items.len() != 3 {
        return None;
    }
    Some([items[0].as_i64()?, items[1].as_i64()?, items[2].as_i64()?])
}

impl Frame {
    /// Reads one line's value. A `snapshot` tree is moved out of `line`.
    pub fn decode(mut line: Value) -> Result<Frame, FrameError> {
        if line.as_obj().is_none() {
            return Err(FrameError::NotAnObject);
        }
        let account = text(&line, "account").unwrap_or_default();
        let kind = text(&line, "type").unwrap_or_default();
        Ok(match kind.as_str() {
            "snapshot" => {
                let snapshot = match &mut line {
                    Value::Obj(map) => map.remove("snapshot").unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                Frame::Snapshot(SnapshotFrame {
                    account,
                    phase: line.get("phase").as_str().map(Phase::parse),
                    status: line.get("status").as_str().map(Status::parse),
                    sent: line.get("sent").as_f64(),
                    snapshot,
                })
            }
            "snapshot_light" => Frame::Light(LightFrame {
                account,
                script: text(&line, "script"),
                status: line.get("status").as_str().map(Status::parse),
                phase: line.get("phase").as_str().map(Phase::parse),
                position: position_of(line.get("position")),
                ticks: line.get("ticks").as_i64(),
                zone: text(&line, "zone"),
                intention: text(&line, "intention"),
                updated: line.get("updated").as_f64(),
                sent: line.get("sent").as_f64(),
                npcs_at: non_null(&line, "npcs_at"),
                players_at: non_null(&line, "players_at"),
            }),
            "task_report" => Frame::TaskReport(ReportFrame { account, report: line.get("report").clone() }),
            "event" => Frame::Event(EventFrame {
                account,
                kind: text(&line, "kind").unwrap_or_default(),
                event: text(&line, "event").unwrap_or_else(|| "unknown".to_string()),
                resolved: line.get("resolved").as_bool().unwrap_or(false),
                tick: line.get("tick").as_i64().unwrap_or(0),
                script: text(&line, "script").unwrap_or_default(),
                args: line.get("args").as_arr().cloned().unwrap_or_default(),
                message: text(&line, "message").unwrap_or_default(),
                at: line.get("at").clone(),
            }),
            _ => return Err(FrameError::UnknownType { account, kind }),
        })
    }

    /// The JSON a worker puts on the wire.
    pub fn encode(&self) -> Value {
        match self {
            Frame::Snapshot(frame) => frame.encode(),
            Frame::Light(frame) => frame.encode(),
            Frame::TaskReport(frame) => frame.encode(),
            Frame::Event(frame) => frame.encode(),
        }
    }

    /// The account the frame is for (empty: the daemon refuses it).
    pub fn account(&self) -> &str {
        match self {
            Frame::Snapshot(frame) => &frame.account,
            Frame::Light(frame) => &frame.account,
            Frame::TaskReport(frame) => &frame.account,
            Frame::Event(frame) => &frame.account,
        }
    }

    /// The session facts of a `snapshot` or light frame; `None` for the
    /// others and for a frame without a `phase` (an older worker).
    pub fn session_facts(&self) -> Option<SessionFacts> {
        match self {
            Frame::Snapshot(frame) => Some(SessionFacts { phase: frame.phase.clone()?, status: frame.status.clone(), sent: frame.sent }),
            Frame::Light(frame) => Some(SessionFacts { phase: frame.phase.clone()?, status: frame.status.clone(), sent: frame.sent.or(frame.updated) }),
            _ => None,
        }
    }
}

fn put(entries: &mut Vec<(&'static str, Value)>, key: &'static str, value: Option<Value>) {
    if let Some(value) = value {
        entries.push((key, value));
    }
}

impl SnapshotFrame {
    pub fn encode(&self) -> Value {
        self.clone().into_value()
    }

    /// `encode` for a frame the caller gives up: the tree moves, no copy.
    pub fn into_value(self) -> Value {
        let mut entries = vec![("type", str_value("snapshot")), ("account", str_value(&self.account))];
        put(&mut entries, "phase", self.phase.as_ref().map(|phase| str_value(phase.as_str())));
        put(&mut entries, "status", self.status.as_ref().map(|status| str_value(status.as_str())));
        put(&mut entries, "sent", self.sent.map(Value::Float));
        entries.push(("snapshot", self.snapshot));
        object(entries)
    }
}

impl LightFrame {
    pub fn encode(&self) -> Value {
        let mut entries = vec![("type", str_value("snapshot_light")), ("account", str_value(&self.account))];
        put(&mut entries, "script", self.script.as_deref().map(str_value));
        put(&mut entries, "status", self.status.as_ref().map(|status| str_value(status.as_str())));
        put(&mut entries, "phase", self.phase.as_ref().map(|phase| str_value(phase.as_str())));
        put(&mut entries, "position", self.position.map(|at| Value::Arr(at.iter().map(|part| Value::Int(*part)).collect())));
        put(&mut entries, "ticks", self.ticks.map(Value::Int));
        put(&mut entries, "zone", self.zone.as_deref().map(str_value));
        put(&mut entries, "intention", self.intention.as_deref().map(str_value));
        put(&mut entries, "updated", self.updated.map(Value::Float));
        put(&mut entries, "sent", self.sent.map(Value::Float));
        put(&mut entries, "npcs_at", self.npcs_at.clone());
        put(&mut entries, "players_at", self.players_at.clone());
        object(entries)
    }

    /// Merge into the cached full state: only the light keys move,
    /// everything else (inv, bank, npcs...) stays from the last full
    /// (`jobs::merge_light` on the typed frame; `phase` is not a state key).
    pub fn merge_into(&self, cached: &mut Value) {
        let Value::Obj(map) = cached else { return };
        let mut set = |key: &str, value: Option<Value>| {
            if let Some(value) = value {
                map.insert(key.to_string(), value);
            }
        };
        set("script", self.script.as_deref().map(str_value));
        set("status", self.status.as_ref().map(|status| str_value(status.as_str())));
        set("position", self.position.map(|at| Value::Arr(at.iter().map(|part| Value::Int(*part)).collect())));
        set("ticks", self.ticks.map(Value::Int));
        set("zone", self.zone.as_deref().map(str_value));
        set("intention", self.intention.as_deref().map(str_value));
        set("updated", self.updated.map(Value::Float));
        set("npcs_at", self.npcs_at.clone());
        set("players_at", self.players_at.clone());
    }
}

impl ReportFrame {
    pub fn encode(&self) -> Value {
        object(vec![("type", str_value("task_report")), ("account", str_value(&self.account)), ("report", self.report.clone())])
    }
}

impl EventFrame {
    pub fn encode(&self) -> Value {
        object(vec![
            ("type", str_value("event")),
            ("account", str_value(&self.account)),
            ("kind", str_value(&self.kind)),
            ("event", str_value(&self.event)),
            ("resolved", Value::Bool(self.resolved)),
            ("tick", Value::Int(self.tick)),
            ("script", str_value(&self.script)),
            ("args", Value::Arr(self.args.clone())),
            ("message", str_value(&self.message)),
            ("at", self.at.clone()),
        ])
    }
}

#[cfg(test)]
#[path = "frame_codec_tests.rs"]
mod tests;
