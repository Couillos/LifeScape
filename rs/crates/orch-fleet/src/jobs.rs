//! One supervised account: persistent session plus its script state.
//!
//! The worker owns the session truth, the
//! job mirrors it for the CLI/dashboard and carries event telemetry.
//! Phases idle|running; statuses starting|idle|running|busy|reconnecting|
//! finished|failed|disconnected|stopped.
//!
//! Naming helpers live here too: the API returns
//! resolved names, never raw ids. Without the gamedata tables only the
//! names already present in the snapshot can be shown, so unknown ids stay
//! blank instead of resolving.

use std::sync::{Arc, Mutex, OnceLock};

use lifescape_json::{Value, object, str_value};
use crate::frame_codec::SessionFacts;
use crate::job_state::{Phase, Status};
use crate::paths;

pub use crate::clock::now_secs;

fn arr_or_empty(value: &Value) -> Value {
    match value {
        Value::Arr(items) => Value::Arr(items.clone()),
        _ => Value::Arr(Vec::new()),
    }
}

fn obj_or_empty(value: &Value) -> Value {
    match value {
        Value::Obj(_) => value.clone(),
        _ => object(vec![]),
    }
}

pub struct JobInner {
    pub script: Option<String>,
    pub script_args: Vec<Value>,
    pub phase: Phase,
    pub paused: bool,
    pub status: Status,
    pub session_pid: Option<u32>,
    pub started: f64,
    /// Manual hold: the daemon runs no rescue while set (see `hold`).
    /// Transient, never persisted: a daemon restart releases every hold.
    pub held: bool,
    pub last_error: Option<Value>,
    /// Error of an earlier run, kept with its time once a later run worked.
    pub previous_error: Option<Value>,
    pub last_event: Option<Value>,
    pub last_report: Option<Value>,
    pub requirements: Option<Value>,
    pub last_code: Option<Value>,
    pub supervise: bool,
    /// Last pushed snapshot (worker push frames): the front reads this,
    /// the disk is only the adoption fallback before the first push arrived.
    pub live: Option<Arc<Value>>,
    /// A task report arrived since the last supervision pass: the single
    /// supervisor drains it immediately (brain trigger hook, E8).
    pub report_pending: bool,
    pub state_written_at: f64,
    /// What the last heartbeat frame said of the session (lot 4.2).
    pub frame: Option<FrameView>,
    /// When `phase`/`status` were last set by a `status` reply, a command
    /// or a death (daemon clock): a frame SENT before it is older news.
    pub phase_at: f64,
    /// Frames counted since the job exists (`FrameView::seq`).
    pub frame_seq: u64,
}

/// The session facts a heartbeat frame carries (lot 4.2): the worker puts
/// `phase` and `status` on every full and light frame, with its send time.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameView {
    pub phase: Phase,
    pub status: Status,
    /// Worker send time (same host clock as the daemon).
    pub sent: f64,
    /// Daemon receive time.
    pub received: f64,
    /// Rank of the frame among the job's frames.
    pub seq: u64,
}

/// What `Job::note_frame` changed on the job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameEffect {
    pub phase_changed: bool,
    pub status_changed: bool,
    /// The phase is now `idle` and was not.
    pub became_idle: bool,
}

/// Columns of the dashboard's bot list, exactly what the spectator table
/// renders (status/name/level/hp/script/zone/position); `account_name` is
/// the humanized account. Goal/task come from the life file, in the web
/// layer. Phase/ticks/errors ride the followed state, not the list.
pub const LIST_FIELDS: &[&str] = &["account", "account_name", "combat", "position", "zone", "status", "paused", "script", "hitpoints"];

/// Rows of `status` in the order the dashboard and the CLI show them:
/// busy bots first, then idle ones, by account. Stable.
pub fn sort_status_rows(rows: &mut [Value]) {
    rows.sort_by(|a, b| {
        let idle = |v: &Value| v.get("status").as_str() == Some("idle");
        (idle(a), a.get("account").as_str().unwrap_or("")).cmp(&(idle(b), b.get("account").as_str().unwrap_or("")))
    });
}

/// The `null` state shared by every reader that found no snapshot.
pub fn null_live() -> Arc<Value> {
    static NULL: OnceLock<Arc<Value>> = OnceLock::new();
    NULL.get_or_init(|| Arc::new(Value::Null)).clone()
}

impl JobInner {
    /// The last pushed snapshot as a pointer (lot 1.5): the 35 to 85 KB
    /// tree is shared, never copied by a reader. A push replaces the
    /// pointer; a light merge copies the tree only while a reader still
    /// holds the previous one (`Arc::make_mut`). Tallied for the brain
    /// thread (`cpu-profile tally`: `live_shared`).
    pub fn live_shared(&self) -> Option<Arc<Value>> {
        crate::cpu_profile::tally("live_shared");
        self.live.clone()
    }

    /// `live_shared`, `null` when no snapshot arrived yet.
    pub fn live_or_null(&self) -> Arc<Value> {
        self.live_shared().unwrap_or_else(null_live)
    }

    /// Deep copy of the snapshot, for the rare reader that needs an owned
    /// tree. Tallied (`live_copy`): the brain's per-pass copies are now 0.
    pub fn live_copy(&self) -> Option<Value> {
        crate::cpu_profile::tally("live_copy");
        self.live.as_deref().cloned()
    }
}

pub struct Job {
    pub account: String,
    pub inner: Mutex<JobInner>,
}

impl Job {
    pub fn new(account: &str) -> Self {
        Job {
            account: account.to_string(),
            inner: Mutex::new(JobInner {
                script: None,
                script_args: Vec::new(),
                phase: Phase::Idle,
                paused: false,
                status: Status::Starting,
                session_pid: None,
                started: now_secs(),
                held: false,
                last_error: None,
                previous_error: None,
                last_event: None,
                last_report: None,
                requirements: None,
                last_code: None,
                supervise: true,
                live: None,
                report_pending: false,
                state_written_at: 0.0,
                frame: None,
                phase_at: 0.0,
                frame_seq: 0,
            }),
        }
    }

    /// Mirror the worker's session truth (phase/script/status/errors).
    pub fn apply_session(&self, session: &Value) {
        if session.as_obj().is_none() {
            return;
        }
        let Ok(mut inner) = self.inner.try_lock() else {
            return;
        };
        if !session.get("phase").is_null() {
            if let Some(phase) = session.get("phase").as_str() {
                inner.phase = Phase::parse(phase);
                inner.phase_at = now_secs();
            }
        }
        if !session.get("script").is_null() {
            inner.script = session.get("script").as_str().map(str::to_string);
        }
        if let Some(args) = session.get("script_args").as_arr() {
            inner.script_args = args.to_vec();
        }
        inner.paused = session.get("paused").as_bool().unwrap_or(inner.paused);
        inner.held = session.get("held").as_bool().unwrap_or(inner.held);
        if let Some(status) = session.get("status").as_str() {
            inner.status = Status::parse(status);
            inner.phase_at = now_secs();
        }
        if !session.get("last_error").is_null() {
            let stamped = crate::error_stamp::adopt(inner.last_error.as_ref(), session.get("last_error"), now_secs(), 0);
            inner.last_error = Some(stamped);
        } else if session.as_obj().is_some_and(|fields| fields.contains_key("last_error")) {
            // The worker cleared it (a later run started or worked).
            let JobInner { last_error, previous_error, .. } = &mut *inner;
            crate::error_stamp::retire(last_error, previous_error);
        }
        if !session.get("last_event").is_null() {
            inner.last_event = Some(session.get("last_event").clone());
        }
        if !session.get("last_code").is_null() {
            inner.last_code = Some(session.get("last_code").clone());
        }
    }

    /// Take the pending task report (lot 4.3): flag and report under ONE
    /// lock, so of several consumers (heartbeat reader, supervisor drain)
    /// exactly one gets it. `None` when nothing is pending or the job is
    /// wedged (the flag then waits for the next consumer).
    pub fn take_report(&self) -> Option<Value> {
        let mut inner = self.inner.try_lock().ok()?;
        if !std::mem::replace(&mut inner.report_pending, false) {
            return None;
        }
        inner.last_report.clone()
    }

    /// The frame view as last recorded (lot 4.2), `None` before the first
    /// frame or when the job is wedged.
    pub fn frame_view(&self) -> Option<FrameView> {
        self.inner.try_lock().ok().and_then(|inner| inner.frame.clone())
    }

    /// Apply the session facts of a heartbeat frame (lot 4.2, decoded by
    /// `frame_codec`, lot 5.2): the view is
    /// recorded and, on a supervised job, the phase and status are applied
    /// unless a `status` reply or a command set them after the frame was
    /// sent. An older worker's frame (no `phase`) is ignored. A wedged job
    /// drops the frame (the next one comes within a second).
    pub fn note_frame(&self, facts: &SessionFacts, now: f64) -> Option<FrameEffect> {
        let phase = facts.phase.clone();
        let status_given = facts.status.is_some();
        let status = facts.status.clone().unwrap_or_default();
        let sent = facts.sent.unwrap_or(now);
        let mut inner = self.inner.try_lock().ok()?;
        inner.frame_seq += 1;
        let seq = inner.frame_seq;
        inner.frame = Some(FrameView { phase: phase.clone(), status: status.clone(), sent, received: now, seq });
        let mut effect = FrameEffect::default();
        if inner.supervise && sent > inner.phase_at {
            if inner.phase != phase {
                effect.became_idle = phase == Phase::Idle;
                effect.phase_changed = true;
                inner.phase = phase;
            }
            if status_given && !status.as_str().is_empty() && inner.status != status {
                effect.status_changed = true;
                inner.status = status;
            }
        }
        Some(effect)
    }

    /// The dashboard's list row (`LIST_FIELDS`), the same values as
    /// `describe()` plus `account_name` sliced to those columns, without
    /// describing the bot: only the small fields are copied out of the
    /// snapshot, never the 35 to 85 KB tree (lot 1.2). A wedged job gives
    /// the contended stub's row (columns it lacks read null).
    pub fn list_row(&self) -> Value {
        let (_merged, partial, _log) = paths::user_paths(&self.account);
        let (status, paused, script, from_live) = match self.inner.try_lock() {
            Ok(guard) => {
                let pick = |bot: &Value| [bot.get("combat").clone(), bot.get("position").clone(), bot.get("zone").clone(), bot.get("hitpoints").clone()];
                let from_live = guard.live.as_ref().map(|live| pick(live));
                let script = match &guard.script {
                    Some(name) => Value::Str(name.clone()),
                    None => Value::Null,
                };
                (str_value(guard.status.as_str()), Value::Bool(guard.paused), script, Some(from_live))
            }
            Err(_) => (str_value("contended"), Value::Null, Value::Null, None),
        };
        let [combat, position, zone, hitpoints] = match from_live {
            // No pushed snapshot yet: the heartbeat file, as `describe` reads it.
            Some(None) => {
                let bot = paths::read_json(&partial).unwrap_or(Value::Null);
                [bot.get("combat").clone(), bot.get("position").clone(), bot.get("zone").clone(), bot.get("hitpoints").clone()]
            }
            Some(Some(fields)) => fields,
            None => [Value::Null, Value::Null, Value::Null, Value::Null],
        };
        object(vec![
            ("account", str_value(&self.account)),
            ("account_name", str_value(&humanize(self.account.as_str()))),
            ("combat", combat),
            ("position", position),
            ("zone", zone),
            ("status", status),
            ("paused", paused),
            ("script", script),
            ("hitpoints", hitpoints),
        ])
    }

    /// Merged daemon + bot view, written to run/users/<account>.json.
    /// Same keys as the heartbeat state. try_lock: a wedged job reports
    /// a contended stub instead of parking the dashboard, the supervisor
    /// or a market sweep on it.
    pub fn describe(&self) -> Value {
        let (_merged, partial, log_path) = paths::user_paths(&self.account);
        // Pushed snapshots first (tens of ms fresh); the heartbeat file is
        // only the last-breath fallback before the first push arrives.
        let (live, inner) = match self.inner.try_lock() {
            Ok(guard) => {
                let live = guard.live_shared();
                let copied = (
                    guard.phase.clone(),
                    guard.script.clone(),
                    guard.script_args.clone(),
                    guard.status.clone(),
                    guard.paused,
                    guard.held,
                    guard.session_pid,
                    guard.started,
                    guard.last_error.clone(),
                    guard.previous_error.clone(),
                    guard.last_event.clone(),
                    guard.last_code.clone(),
                    guard.requirements.clone(),
                );
                (live, copied)
            }
            _ => {
                return object(vec![
                    ("account", str_value(&self.account)),
                    ("status", str_value("contended")),
                ]);
            }
        };
        let (
            phase,
            script,
            script_args,
            status,
            paused,
            held,
            session_pid,
            started,
            last_error,
            previous_error,
            last_event,
            last_code,
            requirements,
        ) = inner;
        let bot = live.unwrap_or_else(|| Arc::new(paths::read_json(&partial).unwrap_or(Value::Null)));
        let mut intention = bot.get("intention").as_str().unwrap_or("").to_string();
        if phase == Phase::Running
            && matches!(
                script.as_deref(),
                Some("rescue_maze" | "rescue_down" | "rescue_fallback")
            )
        {
            intention = format!("rescue {}", script.clone().unwrap_or_default());
        }
        let script = match &script {
            Some(name) => Value::Str(name.clone()),
            None => Value::Null,
        };
        let args = Value::Arr(script_args.clone());
        let pid = match session_pid {
            Some(pid) => Value::Int(pid as i64),
            None => Value::Null,
        };
        object(vec![
            ("account", str_value(&self.account)),
            ("script", script.clone()),
            ("script_args", args.clone()),
            ("args", args),
            ("phase", str_value(phase.as_str())),
            ("paused", Value::Bool(paused)),
            ("held", Value::Bool(held)),
            ("status", str_value(status.as_str())),
            ("session_pid", pid),
            ("started", Value::Float(started)),
            ("intention", str_value(&intention)),
            ("position", bot.get("position").clone()),
            ("zone", bot.get("zone").clone()),
            ("hitpoints", bot.get("hitpoints").clone()),
            ("ticks", bot.get("ticks").clone()),
            ("updated", bot.get("updated").clone()),
            ("combat", bot.get("combat").clone()),
            ("energy", bot.get("energy").clone()),
            ("modal", bot.get("modal").clone()),
            ("hint", bot.get("hint").clone()),
            ("hint_type", bot.get("hint_type").clone()),
            ("flash", bot.get("flash").clone()),
            ("route", arr_or_empty(bot.get("route"))),
            ("route_target", bot.get("route_target").clone()),
            ("trail", arr_or_empty(bot.get("trail"))),
            ("skills", arr_or_empty(bot.get("skills"))),
            // Stats tab "QP:" line of the spectator (null: older worker).
            ("quest_points", bot.get("quest_points").clone()),
            ("worn", arr_or_empty(bot.get("worn"))),
            ("npcs", arr_or_empty(bot.get("npcs"))),
            ("locs", arr_or_empty(bot.get("locs"))),
            ("flora", arr_or_empty(bot.get("flora"))),
            ("objs", arr_or_empty(bot.get("objs"))),
            ("players", arr_or_empty(bot.get("players"))),
            // Light-flow entity positions (1 Hz, `merge_light`): spectate
            // moves the dots from these between full frames. Null after a
            // full frame (never `[]`: the front would wipe the scene).
            ("npcs_at", bot.get("npcs_at").clone()),
            ("players_at", bot.get("players_at").clone()),
            ("inv", arr_or_empty(bot.get("inv"))),
            ("bank", arr_or_empty(bot.get("bank"))),
            ("chat", arr_or_empty(bot.get("chat"))),
            ("messages", arr_or_empty(bot.get("messages"))),
            ("message_ticks", arr_or_empty(bot.get("message_ticks"))),
            ("texts", obj_or_empty(bot.get("texts"))),
            ("log", str_value(&log_path.to_string_lossy())),
            ("last_event", last_event.unwrap_or(Value::Null)),
            (
                "requirements",
                requirements.unwrap_or(Value::Null),
            ),
            ("last_error_ago", crate::error_stamp::age_field(last_error.as_ref(), now_secs())),
            ("last_error", last_error.unwrap_or(Value::Null)),
            ("previous_error", previous_error.unwrap_or(Value::Null)),
            ("last_code", last_code.unwrap_or(Value::Null)),
        ])
    }
}

// -- naming (id to display name) ------------------------------------------

const COSMETIC: &[&str] = &[
    "grass", "twig", "flower", "daisy", "weed", "fern", "petal", "leaf", "leaves", "clover", "rug",
    "dugup",
];

/// Cache names are snake_case: show the words the game shows.
pub fn humanize(name: &str) -> String {
    let mut text = name.replace('_', " ");
    text = text.trim().to_string();
    // Fishing-spot internals look like "0 48 50 saltfish": strip 2-3
    // leading digit groups.
    for groups in [3, 2] {
        let mut rest = text.as_str();
        let mut ok = true;
        for _ in 0..groups {
            match rest.find(' ') {
                Some(space) if rest[..space].bytes().all(|ch| ch.is_ascii_digit()) => {
                    rest = rest[space + 1..].trim_start();
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && !rest.is_empty() && rest != text {
            text = rest.to_string();
            break;
        }
    }
    if text.is_empty() {
        return String::new();
    }
    let mut chars = text.chars();
    let first = chars.next().unwrap().to_uppercase().to_string();
    first + chars.as_str()
}

fn is_cosmetic(name: &str, act: bool) -> bool {
    if act {
        return false;
    }
    let lower = name.to_lowercase();
    COSMETIC.iter().any(|word| lower.contains(word))
}

fn is_flowerbed(name: &str, act: bool) -> bool {
    if act {
        return false;
    }
    name.to_lowercase().contains("flower")
}

/// Fill every name the snapshot left as an id. Mutates and returns it.
/// Unknown ids stay blank: the gamedata tables are not ported, only the
/// names already carried by the snapshot are shown.
pub fn resolve_state(mut state: Value) -> Value {
    let Value::Obj(ref mut map) = state else {
        return state;
    };
    let account = map.get("account").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut npcs = map.get("npcs").and_then(|v| v.as_arr()).cloned().unwrap_or_default();
    for npc in &mut npcs {
        if let Value::Obj(entry) = npc {
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let shown = if name.is_empty() { String::new() } else { name };
            entry.insert("name".to_string(), str_value(&humanize(&shown)));
        }
    }
    let mut hint_name: Option<String> = None;
    if let Some(hint) = map.get("hint").and_then(|v| v.as_arr()) {
        if hint.first().and_then(|item| item.as_str()) == Some("npc") {
            if let Some(nid) = hint.get(1).and_then(|item| item.as_i64()) {
                let mut found: Option<String> = None;
                for npc in &npcs {
                    if npc.get("nid").as_i64() == Some(nid) {
                        found = Some(
                            npc.get("name").as_str().unwrap_or("").to_string(),
                        );
                        break;
                    }
                }
                hint_name = Some(found.unwrap_or_else(|| format!("npc {nid}")));
            }
        } else if hint.first().and_then(|item| item.as_str()) == Some("coord") && hint.len() >= 3 {
            let x = hint[1].as_i64().unwrap_or(0);
            let z = hint[2].as_i64().unwrap_or(0);
            hint_name = Some(format!("tile {x},{z}"));
        }
    }
    let mut locs = map.get("locs").and_then(|v| v.as_arr()).cloned().unwrap_or_default();
    let mut flora = Vec::new();
    let mut kept = Vec::new();
    for mut loc in locs.drain(..) {
        let name = loc.get("name").as_str().unwrap_or("").to_string();
        let act = loc.get("act").as_bool().unwrap_or(false);
        if is_flowerbed(&name, act) {
            flora.push(object(vec![
                ("x", loc.get("x").clone()),
                ("z", loc.get("z").clone()),
                ("id", loc.get("id").clone()),
                ("name", str_value(&humanize(&name))),
            ]));
            continue;
        }
        if let Value::Obj(entry) = &mut loc {
            entry.insert("name".to_string(), str_value(&humanize(&name)));
            entry.insert(
                "cosmetic".to_string(),
                Value::Bool(is_cosmetic(&name, act)),
            );
        }
        kept.push(loc);
    }
    let mut objs = map.get("objs").and_then(|v| v.as_arr()).cloned().unwrap_or_default();
    for obj in &mut objs {
        if let Value::Obj(entry) = obj {
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            entry.insert("name".to_string(), str_value(&humanize(&name)));
        }
    }
    map.insert("npcs".to_string(), Value::Arr(npcs));
    map.insert("locs".to_string(), Value::Arr(kept));
    map.insert("objs".to_string(), Value::Arr(objs));
    if !flora.is_empty() {
        map.insert("flora".to_string(), Value::Arr(flora));
    }
    map.insert("account_name".to_string(), str_value(&humanize(&account)));
    map.insert("modal_name".to_string(), Value::Null);
    map.insert(
        "hint_name".to_string(),
        hint_name.map(|name| str_value(&humanize(&name))).unwrap_or(Value::Null),
    );
    state
}

// -- supervision registry (moved from `supervise`, layering step A5) --------

/// Supervised accounts, each with the fleet that supervises it: the
/// `supervise-all` thread polls it, `live_of` reads snapshots from it.
pub fn supervised() -> &'static Mutex<Vec<(Arc<dyn crate::fleet::Fleet>, Arc<Job>)>> {
    static REGISTRY: OnceLock<Mutex<Vec<(Arc<dyn crate::fleet::Fleet>, Arc<Job>)>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Live snapshot of a supervised account (`None`: not supervised, no
/// snapshot yet, or its job is busy): the market's seller quote prices
/// with the bot's own levels (`market_seller`).
pub fn live_of(account: &str) -> Option<Arc<Value>> {
    let job = supervised().lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().find(|(_, job)| job.account == account).map(|(_, job)| job.clone())?;
    let inner = job.inner.try_lock().ok()?;
    inner.live_shared()
}

/// Merge a light frame into the cached full state: only the light keys
/// move, everything else (inv, bank, npcs…) stays from the last full.
pub fn merge_light(cached: &mut Value, light: &Value) {
    let Value::Obj(map) = cached else { return };
    for key in ["script", "status", "position", "ticks", "zone", "intention", "updated", "npcs_at", "players_at"] {
        let value = light.get(key);
        if !value.is_null() {
            map.insert(key.to_string(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_parity() {
        assert_eq!(humanize("olivia_king"), "Olivia king");
        assert_eq!(humanize("0 48 50 saltfish"), "Saltfish");
        assert_eq!(humanize(""), "");
    }

    /// A held bot stays held across a daemon restart: the worker's
    /// session carries `held`, `restore_jobs` applies it.
    #[test]
    fn restored_session_keeps_its_hold() {
        let job = Job::new("b0");
        job.apply_session(&lifescape_json::object(vec![("held", Value::Bool(true))]));
        assert!(job.inner.lock().unwrap().held, "hold read back from the worker");
    }

    /// The facts of a light frame, read by the production decoder.
    fn frame(phase: &str, status: &str, sent: f64) -> SessionFacts {
        let line = lifescape_json::object(vec![
            ("type", str_value("snapshot_light")),
            ("account", str_value("b1")),
            ("phase", str_value(phase)),
            ("status", str_value(status)),
            ("sent", Value::Float(sent)),
        ]);
        crate::frame_codec::Frame::decode(line).expect("a light frame").session_facts().expect("a phase")
    }

    /// Lot 4.2: a frame sets phase and status, records its view, and is
    /// ignored for the apply when a `status` reply already set them later.
    #[test]
    fn a_frame_sets_phase_and_status_unless_older_than_the_last_reply() {
        let job = Job::new("b1");
        let effect = job.note_frame(&frame("running", "running", 100.0), 100.1).expect("read");
        assert!(effect.phase_changed && effect.status_changed && !effect.became_idle);
        assert_eq!(job.inner.lock().unwrap().phase, Phase::Running);
        let effect = job.note_frame(&frame("idle", "idle", 101.0), 101.1).expect("read");
        assert!(effect.became_idle);
        assert_eq!(job.inner.lock().unwrap().status, Status::Idle);
        // A reply taken after the frame was sent wins over it.
        job.inner.lock().unwrap().phase_at = 200.0;
        job.apply_session(&lifescape_json::object(vec![("phase", str_value("running")), ("status", str_value("running"))]));
        job.inner.lock().unwrap().phase_at = 1.0e12;
        let effect = job.note_frame(&frame("idle", "idle", 150.0), 201.0).expect("read");
        assert_eq!(effect, FrameEffect::default(), "an older frame changes nothing");
        assert_eq!(job.inner.lock().unwrap().phase, Phase::Running);
        let view = job.frame_view().expect("view kept");
        assert_eq!((view.phase.as_str(), view.seq), ("idle", 3), "the view still records every frame");
        // An unsupervised (closed, dead) job only records.
        let closed = Job::new("b2");
        closed.inner.lock().unwrap().supervise = false;
        assert_eq!(closed.note_frame(&frame("running", "running", 10.0), 10.0), Some(FrameEffect::default()));
        assert_eq!(closed.inner.lock().unwrap().phase, Phase::Idle);
        // A frame without a phase (older worker) is not a frame view.
        let bare = lifescape_json::object(vec![("type", str_value("snapshot_light")), ("account", str_value("b1"))]);
        assert_eq!(crate::frame_codec::Frame::decode(bare).expect("decodes").session_facts(), None);
    }

    #[test]
    fn describe_has_all_keys() {
        let job = Job::new("t01");
        let state = job.describe();
        for key in [
            "account", "script", "script_args", "args", "phase", "paused", "held", "status", "session_pid",
            "started", "intention", "position", "zone", "hitpoints", "ticks", "updated", "combat",
            "energy", "modal", "hint", "hint_type", "flash", "route", "route_target", "skills",
            "quest_points", "worn", "npcs", "npcs_at", "locs", "flora", "objs", "players", "players_at", "inv", "bank", "chat", "messages",
            "message_ticks", "texts", "log", "last_event", "requirements", "last_error",
            "last_code", "trail",
        ] {
            assert!(state.as_obj().unwrap().contains_key(key), "missing {key}");
        }
    }

    /// A cleared worker error retires to `previous_error` with its time;
    /// the current one is stamped and shows its age.
    #[test]
    fn cleared_worker_error_moves_to_previous() {
        let job = Job::new("t02");
        let blocked = object(vec![("type", str_value("Blocked")), ("message", str_value("blocked, missing: iron_bar"))]);
        job.apply_session(&object(vec![("last_error", blocked)]));
        let state = job.describe();
        assert!(state.get("last_error").get("at_unix").as_f64().is_some(), "stamped with its time");
        assert_eq!(state.get("last_error_ago").as_str(), Some("il y a 0 s"));
        job.apply_session(&object(vec![("held", Value::Bool(false))]));
        assert!(!job.describe().get("last_error").is_null(), "a partial session leaves it alone");
        job.apply_session(&object(vec![("last_error", Value::Null)]));
        let state = job.describe();
        assert!(state.get("last_error").is_null(), "a later clean run clears it");
        assert_eq!(state.get("previous_error").get("type").as_str(), Some("Blocked"));
    }

    /// Lot 1.5: readers take the pointer, never a copy; a light merge
    /// copies the tree only while a reader still holds the old pointer
    /// (the reader keeps its consistent snapshot), else merges in place.
    #[test]
    fn readers_share_the_snapshot_and_a_merge_copies_only_for_them() {
        let job = Job::new("t03");
        let full = object(vec![
            ("position", Value::Arr(vec![Value::Int(0), Value::Int(3200), Value::Int(3200)])),
            ("trail", Value::Arr((0..400).map(Value::Int).collect())),
        ]);
        job.inner.lock().unwrap().live = Some(Arc::new(full));
        let first = job.inner.lock().unwrap().live_or_null();
        let second = job.inner.lock().unwrap().live_shared().expect("live");
        assert!(Arc::ptr_eq(&first, &second), "two readers, one tree");
        assert!(Arc::ptr_eq(&first, job.inner.lock().unwrap().live.as_ref().unwrap()));
        let light = object(vec![("position", Value::Arr(vec![Value::Int(0), Value::Int(3201), Value::Int(3200)]))]);
        let merge = |job: &Job| {
            let mut inner = job.inner.lock().unwrap();
            let Some(cached) = inner.live.as_mut() else { return };
            if let (Value::Obj(map), Value::Obj(update)) = (Arc::make_mut(cached), &light) {
                for (key, value) in update {
                    map.insert(key.clone(), value.clone());
                }
            }
        };
        merge(&job);
        assert_eq!(first.get("position").as_arr().map(|cells| cells[1].as_i64()), Some(Some(3200)), "a reader's snapshot never changes under it");
        let now = job.inner.lock().unwrap().live_or_null();
        assert_eq!(now.get("position").as_arr().map(|cells| cells[1].as_i64()), Some(Some(3201)));
        assert!(!Arc::ptr_eq(&first, &now));
        drop((first, second));
        let before = Arc::as_ptr(&now);
        drop(now);
        merge(&job);
        let after = Arc::as_ptr(job.inner.lock().unwrap().live.as_ref().unwrap());
        assert_eq!(before, after, "no reader holds it: merged in place, no copy");
        assert!(Arc::ptr_eq(&null_live(), &null_live()), "the null state is one shared value");
    }
}
