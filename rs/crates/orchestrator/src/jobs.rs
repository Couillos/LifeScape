//! One supervised account: persistent session plus its script state.
//!
//! Mirrors orchestration/jobs.py: the worker owns the session truth, the
//! job mirrors it for the CLI/dashboard and carries rescue bookkeeping.
//! Phases idle|running; statuses starting|idle|running|busy|reconnecting|
//! finished|failed|disconnected|stopped.
//!
//! Naming helpers (orchestration/naming.py) live here too: the API returns
//! resolved names, never raw ids. Without the gamedata tables only the
//! names already present in the snapshot can be shown, so unknown ids stay
//! blank instead of resolving.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{Value, object, str_value};
use crate::paths;

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|span| span.as_secs_f64())
        .unwrap_or(0.0)
}

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
    pub phase: String,
    pub paused: bool,
    pub status: String,
    pub session_pid: Option<u32>,
    pub started: f64,
    pub last_error: Option<Value>,
    pub last_event: Option<Value>,
    pub requirements: Option<Value>,
    pub last_code: Option<Value>,
    pub interrupted: Option<(String, Vec<Value>)>,
    pub rescue_for: Option<String>,
    pub worker_rescue_at: Option<Value>,
    pub supervise: bool,
    pub log_offset: u64,
    pub state_written_at: f64,
    pub event_last_pos: Option<(i64, i64)>,
    pub event_detect_at: f64,
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
                phase: "idle".to_string(),
                paused: false,
                status: "starting".to_string(),
                session_pid: None,
                started: now_secs(),
                last_error: None,
                last_event: None,
                requirements: None,
                last_code: None,
                interrupted: None,
                rescue_for: None,
                worker_rescue_at: None,
                supervise: true,
                log_offset: 0,
                state_written_at: 0.0,
                event_last_pos: None,
                event_detect_at: 0.0,
            }),
        }
    }

    /// Mirror the worker's session truth (phase/script/status/errors).
    pub fn apply_session(&self, session: &Value) {
        if session.as_obj().is_none() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if !session.get("phase").is_null() {
            if let Some(phase) = session.get("phase").as_str() {
                inner.phase = phase.to_string();
            }
        }
        if !session.get("script").is_null() {
            inner.script = session.get("script").as_str().map(str::to_string);
        }
        if let Some(args) = session.get("script_args").as_arr() {
            inner.script_args = args.to_vec();
        }
        inner.paused = session.get("paused").as_bool().unwrap_or(inner.paused);
        if let Some(status) = session.get("status").as_str() {
            inner.status = status.to_string();
        }
        if !session.get("last_error").is_null() {
            inner.last_error = Some(session.get("last_error").clone());
        }
        if !session.get("last_event").is_null() {
            inner.last_event = Some(session.get("last_event").clone());
        }
        if !session.get("last_code").is_null() {
            inner.last_code = Some(session.get("last_code").clone());
        }
    }

    /// Merged daemon + bot view, written to run/users/<account>.json.
    /// Same keys as jobs.py describe().
    pub fn describe(&self) -> Value {
        let (_merged, partial, log_path) = paths::user_paths(&self.account);
        let bot = paths::read_json(&partial).unwrap_or(Value::Null);
        let inner = self.inner.lock().unwrap();
        let mut intention = bot.get("intention").as_str().unwrap_or("").to_string();
        if inner.phase == "running"
            && matches!(
                inner.script.as_deref(),
                Some("rescue_maze" | "rescue_down" | "rescue_fallback")
            )
        {
            intention = format!("rescue {}", inner.script.clone().unwrap_or_default());
        }
        let script = match &inner.script {
            Some(name) => Value::Str(name.clone()),
            None => Value::Null,
        };
        let args = Value::Arr(inner.script_args.clone());
        let pid = match inner.session_pid {
            Some(pid) => Value::Int(pid as i64),
            None => Value::Null,
        };
        object(vec![
            ("account", str_value(&self.account)),
            ("script", script.clone()),
            ("script_args", args.clone()),
            ("args", args),
            ("phase", str_value(&inner.phase)),
            ("paused", Value::Bool(inner.paused)),
            ("status", str_value(&inner.status)),
            ("session_pid", pid),
            ("started", Value::Float(inner.started)),
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
            ("skills", arr_or_empty(bot.get("skills"))),
            ("worn", arr_or_empty(bot.get("worn"))),
            ("npcs", arr_or_empty(bot.get("npcs"))),
            ("locs", arr_or_empty(bot.get("locs"))),
            ("flora", arr_or_empty(bot.get("flora"))),
            ("objs", arr_or_empty(bot.get("objs"))),
            ("players", arr_or_empty(bot.get("players"))),
            ("inv", arr_or_empty(bot.get("inv"))),
            ("bank", arr_or_empty(bot.get("bank"))),
            ("chat", arr_or_empty(bot.get("chat"))),
            ("messages", arr_or_empty(bot.get("messages"))),
            ("message_ticks", arr_or_empty(bot.get("message_ticks"))),
            ("texts", obj_or_empty(bot.get("texts"))),
            ("log", str_value(&log_path.to_string_lossy())),
            ("last_event", inner.last_event.clone().unwrap_or(Value::Null)),
            (
                "requirements",
                inner.requirements.clone().unwrap_or(Value::Null),
            ),
            ("last_error", inner.last_error.clone().unwrap_or(Value::Null)),
            ("last_code", inner.last_code.clone().unwrap_or(Value::Null)),
        ])
    }
}

// -- naming (orchestration/naming.py) -------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_parity() {
        assert_eq!(humanize("olivia_king"), "Olivia king");
        assert_eq!(humanize("0 48 50 saltfish"), "Saltfish");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn describe_has_all_keys() {
        let job = Job::new("t01");
        let state = job.describe();
        for key in [
            "account", "script", "script_args", "args", "phase", "paused", "status", "session_pid",
            "started", "intention", "position", "zone", "hitpoints", "ticks", "updated", "combat",
            "energy", "modal", "hint", "hint_type", "flash", "route", "route_target", "skills",
            "worn", "npcs", "locs", "flora", "objs", "players", "inv", "bank", "chat", "messages",
            "message_ticks", "texts", "log", "last_event", "requirements", "last_error",
            "last_code",
        ] {
            assert!(state.as_obj().unwrap().contains_key(key), "missing {key}");
        }
    }
}
