#![forbid(unsafe_code)]
//! Persistent game session: one connected account, many scripts over time.
//!
//! Rust port of `orchestration/session_worker.py`: the same unix-socket
//! JSON-lines protocol (ping/status/methods/run/unload/pause/resume/call/
//! close), the same `describe` keys (`ok`/`error`/`session`), the same
//! `run/session-<account>.sock` + `.ready` paths and env (`LIFESCAPE_USER`,
//! `LIFESCAPE_STATE`, `LIFESCAPE_NO_LOGIN`), and the same log markers
//! (`=== session ... ===`, `@@EVENT` lines intact, `[HH:MM:SS]` prefixes on
//! worker lines).
//!
//! Shape of the port: the script runs on one dedicated thread that owns the
//! pump while it lives; the main thread serves the socket and pumps only
//! when idle. Reads come from the last heartbeat snapshot, never from the
//! live bot off its thread. Writes run on the idle pump thread (the
//! safe-point); while a script holds the bot they return `busy`, like the
//! Python wait hitting its 500-tick bound.
//!
//! shortcut: no cooperative stop flag in the SDK, so unload cannot freeze
//! a running walk mid-leg; it waits for the script thread up to
//! UNLOAD_TICKS then reports the orphan and lets it finish. Revisit when
//! the SDK grows a stop hook.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lifescape_sdk::client::Snapshot;
use lifescape_sdk::Bot;
use lifescape_scripts::randomevent;

use crate::json::{self, Value, object, str_value};
use crate::paths;
use crate::session_calls::{execute_write, method_lists, read_from_snapshot};
use crate::session_heartbeat::heartbeat_value;

/// A queued SDK call waits at most this long (in ticks) for a safe-point.
const SAFE_POINT_TICKS: u64 = 500;
/// An unload waits this long (in ticks) for the script thread to notice.
const UNLOAD_TICKS: u64 = 250;
/// Process guard when ticks are frozen (offline/CPU spin), in seconds.
const WALL_BACKSTOP: f64 = 30.0;
/// Heartbeat freshness gate, in ticks (same pacing as the Python writer).
const HEARTBEAT_TICKS: u64 = 10;
/// Idle pump cadence, in milliseconds.
const IDLE_POLL_MS: u64 = 100;

pub(crate) fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|span| span.as_secs_f64()).unwrap_or(0.0)
}

fn clock_hhmmss() -> String {
    let secs = now_secs() as u64;
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

/// Worker framework line on stdout (which the daemon redirects to the
/// session log): timestamped, while `@@EVENT` lines pass through intact so
/// the daemon's event tail keeps working. Script `println!` output passes
/// through verbatim; only these framework lines carry the prefix.
fn worker_out(line: &str) {
    for chunk in line.split('\n') {
        if chunk.is_empty() || chunk.starts_with("@@EVENT ") {
            println!("{chunk}");
        } else {
            println!("[{}] {chunk}", clock_hhmmss());
        }
    }
}

fn append_log(log_path: &std::path::Path, line: &str) {
    if let Ok(mut handle) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(handle, "{line}");
    }
}

fn engine_dir() -> String {
    if let Ok(dir) = std::env::var("LIFESCAPE_ENGINE_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    let root = paths::root();
    let sibling = root.join("../lostcity/engine");
    if sibling.is_dir() {
        return sibling.to_string_lossy().to_string();
    }
    "../lostcity/engine".to_string()
}

fn server_host() -> String {
    std::env::var("LIFESCAPE_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn server_port() -> u16 {
    std::env::var("LIFESCAPE_SERVER_PORT").ok().and_then(|raw| raw.parse().ok()).unwrap_or(43594)
}

/// One queued write: the idle pump thread fills `outcome` and sets `done`.
struct QueuedCall {
    method: String,
    args: Vec<Value>,
    started: AtomicBool,
    done: AtomicBool,
    outcome: Mutex<Option<Result<Value, String>>>,
}

struct SessionInner {
    phase: String,
    script: Option<String>,
    script_args: Vec<String>,
    paused: bool,
    connected: bool,
    status: String,
    last_error: Option<Value>,
    last_event: Option<Value>,
    last_code: Option<Value>,
    closing: bool,
    script_alive: bool,
    last_watch_tick: u64,
    last_heartbeat_tick: u64,
    last_msg_logged: usize,
}

struct Shared {
    account: String,
    log_path: PathBuf,
    state_path: PathBuf,
    inner: Mutex<SessionInner>,
    bot: Mutex<Bot>,
    heartbeat: Mutex<Option<Snapshot>>,
    queue: Mutex<VecDeque<Arc<QueuedCall>>>,
    stop_requested: AtomicBool,
}

impl Shared {
    fn session_pid() -> u32 {
        std::process::id()
    }

    fn log(&self, line: &str) {
        append_log(&self.log_path, line);
    }

    fn describe(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        let ticks = self.bot.try_lock().map(|bot| bot.world.ticks as i64).unwrap_or_else(|_| {
            inner.last_heartbeat_tick as i64
        });
        let script = match &inner.script {
            Some(name) => str_value(name),
            None => Value::Null,
        };
        object(vec![
            ("account", str_value(&self.account)),
            ("phase", str_value(&inner.phase)),
            ("script", script),
            (
                "script_args",
                Value::Arr(inner.script_args.iter().map(|arg| str_value(arg)).collect()),
            ),
            ("paused", Value::Bool(inner.paused)),
            ("status", str_value(&inner.status)),
            ("session_pid", Value::Int(Self::session_pid() as i64)),
            ("last_error", inner.last_error.clone().unwrap_or(Value::Null)),
            ("last_event", inner.last_event.clone().unwrap_or(Value::Null)),
            ("last_code", inner.last_code.clone().unwrap_or(Value::Null)),
            ("ticks", Value::Int(ticks)),
        ])
    }
}

// -- heartbeat ------------------------------------------------------------

/// Throttled heartbeat for a bot borrowed by the script thread: same
/// payload as `write_heartbeat`, no bot lock needed. Installed as the
/// bot's pump hook so spectate/route/calls stay live during runs.
/// Returns false once the operator stops the run (unload/close): the pump
/// aborts and the script unwinds instead of finishing its action.
fn publish_tick(shared: &Shared, bot: &lifescape_sdk::Bot) -> bool {
    let tick = bot.world.ticks;
    {
        let mut inner = shared.inner.lock().unwrap();
        if bot.world.messages.len() < inner.last_msg_logged {
            inner.last_msg_logged = 0;
        }
        while inner.last_msg_logged < bot.world.messages.len() {
            let index = inner.last_msg_logged;
            let text = bot.world.messages.get(index).cloned().unwrap_or_default();
            let at = bot.world.message_ticks.get(index).copied().unwrap_or(tick);
            worker_out(&format!("msg [t={at}] {text}"));
            inner.last_msg_logged = index + 1;
        }
    }
    let last = shared.inner.lock().unwrap().last_heartbeat_tick;
    if tick.saturating_sub(last) >= HEARTBEAT_TICKS {
        let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
        let state = heartbeat_value(bot, &shared.account, &script);
        if paths::write_json(&shared.state_path, &state).is_err() {
            return !shared.stop_requested.load(Ordering::SeqCst);
        }
        *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
        shared.inner.lock().unwrap().last_heartbeat_tick = tick;
    }
    !shared.stop_requested.load(Ordering::SeqCst)
}

/// Atomic heartbeat for `run/users/<account>.bot.json`: the same keys the
/// Python `_write_state` publishes, so `Job::describe` keeps working.
fn write_heartbeat(shared: &Arc<Shared>) {
    let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
    let Ok(bot) = shared.bot.try_lock() else { return };
    let state = heartbeat_value(&bot, &shared.account, &script);
    if paths::write_json(&shared.state_path, &state).is_err() {
        worker_out(&format!("state write failed ({})", shared.state_path.display()));
        return;
    }
    *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
    shared.inner.lock().unwrap().last_heartbeat_tick = bot.world.ticks;
}


// -- commands --------------------------------------------------------------

fn current_ticks(shared: &Shared) -> u64 {
    shared.bot.try_lock().map(|bot| bot.world.ticks).unwrap_or_else(|_| shared.inner.lock().unwrap().last_heartbeat_tick)
}

fn cmd_run(shared: &Arc<Shared>, script: &str, args: &[Value]) -> Value {
    let known: Vec<&str> = lifescape_scripts::names();
    {
        let inner = shared.inner.lock().unwrap();
        if matches!(inner.status.as_str(), "busy" | "reconnecting" | "disconnected") {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("{} is {}", shared.account, inner.status)))]);
        }
        if inner.script_alive {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("busy: {} is running", inner.script.as_deref().unwrap_or("")))),
            ]);
        }
        if !known.iter().any(|name| *name == script) {
            return object(vec![(
                "ok",
                Value::Bool(false),
            ), (
                "error",
                str_value(&format!("unknown script {script:?} (known: {})", {
                    let mut sorted = known.clone();
                    sorted.sort_unstable();
                    sorted.join(", ")
                })),
            )]);
        }
    }
    let string_args: Vec<String> = args
        .iter()
        .map(|arg| match arg {
            Value::Str(text) => text.clone(),
            other => json::stringify(other),
        })
        .collect();
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.script = Some(script.to_string());
        inner.script_args = string_args.clone();
        inner.phase = "running".to_string();
        inner.status = "running".to_string();
        inner.last_code = None;
        inner.last_error = None;
        inner.script_alive = true;
    }
    shared.stop_requested.store(false, Ordering::SeqCst);
    std::env::set_var("LIFESCAPE_SCRIPT", script);
    shared.log(&format!("=== session run {} {script} ===", shared.account));
    let worker = Arc::clone(shared);
    let script_name = script.to_string();
    std::thread::spawn(move || run_script_thread(&worker, &script_name, &string_args));
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
}

fn script_error_type(error: &lifescape_scripts::ScriptError) -> &'static str {
    match error {
        lifescape_scripts::ScriptError::Sdk(_) => "Sdk",
        lifescape_scripts::ScriptError::NotPorted(_) => "NotPorted",
        lifescape_scripts::ScriptError::Usage(_) => "Usage",
    }
}

fn run_script_thread(shared: &Arc<Shared>, script: &str, args: &[String]) {
    let is_rescue = script.starts_with("rescue_");
    let outcome: (String, Option<Value>, Option<Value>) = (|| {
        let mut bot = shared.bot.lock().unwrap();
        match lifescape_scripts::run(script, &mut bot, args) {
            Ok(code) => {
                if code == 0 {
                    ("finished".to_string(), Some(Value::Int(i64::from(code))), None)
                } else {
                    let (level, pos_x, pos_z) = bot.position();
                    let error = object(vec![
                        ("where", str_value(script)),
                        ("type", str_value("exit")),
                        ("message", str_value(&format!("run returned {code}"))),
                        ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                    ]);
                    ("failed".to_string(), Some(Value::Int(i64::from(code))), Some(error))
                }
            }
            Err(script_error) => {
                use lifescape_sdk::error::Error as SdkError;
                let reconnect = matches!(&script_error, lifescape_scripts::ScriptError::Sdk(SdkError::Io(_)) | lifescape_scripts::ScriptError::Sdk(SdkError::Login(_)));
                if reconnect {
                    return ("reconnect".to_string(), None, None);
                }
                if matches!(&script_error, lifescape_scripts::ScriptError::Sdk(SdkError::Interrupted(_))) {
                    // Operator unload/close: back to idle, no rescue event.
                    worker_out(&format!("{script} interrupted at {:?}", bot.position()));
                    return ("idle".to_string(), None, None);
                }
                let (level, pos_x, pos_z) = bot.position();
                let message = script_error.to_string();
                worker_out(&format!(
                    "@@EVENT {{\"kind\":\"script_error\",\"script\":\"{script}\",\"type\":\"{}\",\"message\":{},\"at\":[{level},{pos_x},{pos_z}],\"target\":null}}",
                    script_error_type(&script_error),
                    json::stringify(&str_value(&message)),
                ));
                let error = object(vec![
                    ("where", str_value(script)),
                    ("type", str_value(script_error_type(&script_error))),
                    ("message", str_value(&message)),
                    ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                ]);
                if is_rescue {
                    ("failed".to_string(), None, Some(error))
                } else {
                    ("idle".to_string(), None, Some(error))
                }
            }
        }
    })();
    if outcome.0 == "reconnect" && !shared.inner.lock().unwrap().closing {
        reconnect_flow(shared);
        let mut inner = shared.inner.lock().unwrap();
        inner.script_alive = false;
        inner.phase = "idle".to_string();
        inner.script = None;
        inner.script_args = Vec::new();
        if inner.status == "reconnecting" {
            inner.status = "idle".to_string();
        }
        std::env::set_var("LIFESCAPE_SCRIPT", "");
        write_heartbeat(shared);
        return;
    }
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.script_alive = false;
        inner.phase = "idle".to_string();
        inner.script = None;
        inner.script_args = Vec::new();
        std::env::set_var("LIFESCAPE_SCRIPT", "");
        match outcome.0.as_str() {
            "finished" => {
                inner.status = "finished".to_string();
                inner.last_code = outcome.1;
            }
            "failed" => {
                inner.status = "failed".to_string();
                inner.last_code = outcome.1;
                inner.last_error = outcome.2.clone();
                if let Some(error) = outcome.2 {
                    shared.log(&format!("=== session failed {} {} ===", shared.account, json::stringify(&error)));
                }
            }
            _ => {
                inner.status = "idle".to_string();
                if outcome.2.is_some() {
                    inner.last_error = outcome.2;
                }
            }
        }
    }
    shared.stop_requested.store(false, Ordering::SeqCst);
    write_heartbeat(shared);
}

fn cmd_unload(shared: &Arc<Shared>) -> Value {
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.paused = false;
        if !inner.script_alive {
            inner.phase = "idle".to_string();
            inner.script = None;
            inner.script_args = Vec::new();
            if inner.status == "running" || inner.status == "finished" {
                inner.status = "idle".to_string();
            }
            drop(inner);
            return object(vec![("ok", Value::Bool(true)), ("session", shared.describe())]);
        }
        inner.status = "busy".to_string();
    }
    shared.log(&format!("=== session unload {} ===", shared.account));
    shared.stop_requested.store(true, Ordering::SeqCst);
    let start_tick = current_ticks(shared);
    let start_wall = now_secs();
    while shared.inner.lock().unwrap().script_alive {
        if current_ticks(shared).saturating_sub(start_tick) >= UNLOAD_TICKS || now_secs() - start_wall > WALL_BACKSTOP {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if shared.inner.lock().unwrap().script_alive {
        worker_out("script thread survived unload, it finishes on its own");
        return object(vec![("ok", Value::Bool(true)), ("session", shared.describe())]);
    }
    {
        let mut queued = shared.queue.lock().unwrap();
        for call in queued.drain(..) {
            *call.outcome.lock().unwrap() = Some(Err("busy: unload in progress".to_string()));
            call.done.store(true, Ordering::SeqCst);
        }
    }
    shared.stop_requested.store(false, Ordering::SeqCst);
    if shared.inner.lock().unwrap().connected {
        let pump_outcome = shared.bot.lock().unwrap().pump(1).map(|_| ());
        if pump_outcome.is_err() {
            reconnect_flow(shared);
        }
    }
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.phase = "idle".to_string();
        inner.script = None;
        inner.script_args = Vec::new();
        std::env::set_var("LIFESCAPE_SCRIPT", "");
        if inner.status == "running" || inner.status == "busy" {
            inner.status = "idle".to_string();
        }
    }
    write_heartbeat(shared);
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
}

fn cmd_pause(shared: &Arc<Shared>) -> Value {
    {
        let mut inner = shared.inner.lock().unwrap();
        if !inner.script_alive {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("nothing to pause on {}", shared.account)))]);
        }
        if inner.paused {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("{} is already paused", shared.account)))]);
        }
        inner.paused = true;
    }
    shared.log(&format!("=== session pause {} ===", shared.account));
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
}

fn cmd_resume(shared: &Arc<Shared>) -> Value {
    {
        let mut inner = shared.inner.lock().unwrap();
        if !inner.paused {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("{} is not paused", shared.account)))]);
        }
        inner.paused = false;
    }
    shared.log(&format!("=== session resume {} ===", shared.account));
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
}

fn cmd_call(shared: &Arc<Shared>, method: &str, args: &[Value], admin: bool) -> Value {
    shared.log(&format!("=== session call {} {method} ===", shared.account));
    let (reads, writes) = method_lists(admin);
    if reads.iter().any(|name| name == method) {
        return match shared.heartbeat.lock().unwrap().clone() {
            Some(snapshot) => match read_from_snapshot(&snapshot, method) {
                Ok(result) => object(vec![("ok", Value::Bool(true)), ("result", result)]),
                Err(error) => object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]),
            },
            None => object(vec![("ok", Value::Bool(false)), ("error", str_value("no snapshot yet"))]),
        };
    }
    if !writes.iter().any(|name| name == method) {
        if method == "cheat" && !admin {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("forbidden: cheat is staff only"))]);
        }
        return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("unknown method {method:?}")))]);
    }
    {
        let inner = shared.inner.lock().unwrap();
        if matches!(inner.status.as_str(), "busy" | "reconnecting" | "disconnected") {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("busy: session is {}", inner.status)))]);
        }
        if shared.stop_requested.load(Ordering::SeqCst) {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("busy: unload in progress"))]);
        }
        if inner.script_alive {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("busy: script did not yield within 500 ticks"))]);
        }
    }
    let call = Arc::new(QueuedCall {
        method: method.to_string(),
        args: args.to_vec(),
        started: AtomicBool::new(false),
        done: AtomicBool::new(false),
        outcome: Mutex::new(None),
    });
    shared.queue.lock().unwrap().push_back(Arc::clone(&call));
    let start_tick = current_ticks(shared);
    let start_wall = now_secs();
    loop {
        if call.done.load(Ordering::SeqCst) {
            break;
        }
        if shared.stop_requested.load(Ordering::SeqCst) || shared.inner.lock().unwrap().closing {
            shared.queue.lock().unwrap().retain(|queued| !Arc::ptr_eq(queued, &call));
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("busy: unload in progress"))]);
        }
        if current_ticks(shared).saturating_sub(start_tick) >= SAFE_POINT_TICKS || now_secs() - start_wall > WALL_BACKSTOP {
            if call.started.load(Ordering::SeqCst) {
                return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("call {method} stalled: ticks frozen")))]);
            }
            shared.queue.lock().unwrap().retain(|queued| !Arc::ptr_eq(queued, &call));
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("busy: script did not yield within 500 ticks"))]);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let outcome = call.outcome.lock().unwrap().take();
    match outcome {
        Some(Ok(result)) => object(vec![("ok", Value::Bool(true)), ("result", result)]),
        Some(Err(error)) => object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]),
        None => object(vec![("ok", Value::Bool(false)), ("error", str_value("busy: script did not yield within 500 ticks"))]),
    }
}

fn drain_queue(shared: &Arc<Shared>) {
    let call = shared.queue.lock().unwrap().pop_front();
    let Some(call) = call else { return };
    call.started.store(true, Ordering::SeqCst);
    let outcome = (|| {
        let mut bot = shared.bot.lock().unwrap();
        let result = execute_write(&mut bot, &call.method, &call.args)?;
        let _ = bot.pump(1);
        Ok(result)
    })();
    let _ = match outcome {
        Ok(result) => {
            *call.outcome.lock().unwrap() = Some(Ok(result));
            write_heartbeat(shared);
            Ok(())
        }
        Err(error) => {
            *call.outcome.lock().unwrap() = Some(Err(error));
            Ok::<(), ()>(())
        }
    };
    call.done.store(true, Ordering::SeqCst);
}

fn cmd_close(shared: &Arc<Shared>) -> Value {
    let _ = cmd_unload(shared);
    shared.log(&format!("=== session close {} ===", shared.account));
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.closing = true;
        inner.status = "stopped".to_string();
    }
    shared.bot.lock().unwrap().close();
    let session = shared.describe();
    let worker = Arc::clone(shared);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        worker.inner.lock().unwrap().closing = true;
    });
    object(vec![("ok", Value::Bool(true)), ("session", session)])
}

fn dispatch(shared: &Arc<Shared>, request: &Value) -> Value {
    match request.get("cmd").as_str().unwrap_or("") {
        "ping" => object(vec![("ok", Value::Bool(true))]),
        "status" => object(vec![("ok", Value::Bool(true)), ("session", shared.describe())]),
        "methods" => {
            let (reads, writes) = method_lists(request.get("admin").as_bool().unwrap_or(false));
            object(vec![
                ("ok", Value::Bool(true)),
                ("reads", Value::Arr(reads.into_iter().map(|name| str_value(&name)).collect())),
                ("writes", Value::Arr(writes.into_iter().map(|name| str_value(&name)).collect())),
            ])
        }
        "run" => cmd_run(shared, request.get("script").as_str().unwrap_or(""), &request.get("args").as_arr().cloned().unwrap_or_default()),
        "unload" => cmd_unload(shared),
        "pause" => cmd_pause(shared),
        "resume" => cmd_resume(shared),
        "call" => cmd_call(
            shared,
            request.get("method").as_str().unwrap_or(""),
            &request.get("args").as_arr().cloned().unwrap_or_default(),
            request.get("admin").as_bool().unwrap_or(false),
        ),
        "close" => cmd_close(shared),
        unknown => object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("unknown command {unknown:?}")))]),
    }
}

// -- network ---------------------------------------------------------------

fn reconnect_flow(shared: &Arc<Shared>) {
    {
        let mut inner = shared.inner.lock().unwrap();
        if inner.closing {
            return;
        }
        inner.status = "reconnecting".to_string();
    }
    let outcome = shared.bot.lock().unwrap().relogin().map_err(|error| error.to_string());
    match outcome {
        Ok(()) => {
            let mut inner = shared.inner.lock().unwrap();
            inner.connected = true;
            if inner.status == "reconnecting" {
                inner.status = "idle".to_string();
            }
            drop(inner);
            shared.bot.lock().unwrap().intend("reconnected");
            write_heartbeat(shared);
        }
        Err(error) => {
            let (level, pos_x, pos_z) = shared.bot.lock().unwrap().position();
            let mut inner = shared.inner.lock().unwrap();
            inner.status = "disconnected".to_string();
            inner.last_error = Some(object(vec![
                ("where", str_value("relogin")),
                ("type", str_value("relogin")),
                ("message", str_value(&error)),
                ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
            ]));
            shared.log(&format!("=== session disconnected {} {error} ===", shared.account));
        }
    }
}

fn idle_pump(shared: &Arc<Shared>) {
    {
        let inner = shared.inner.lock().unwrap();
        if inner.script_alive || !inner.connected {
            return;
        }
        if matches!(inner.status.as_str(), "busy" | "reconnecting" | "disconnected" | "starting") {
            return;
        }
        if shared.stop_requested.load(Ordering::SeqCst) || inner.paused {
            return;
        }
    }
    let pump_outcome = shared.bot.lock().unwrap().pump(1).map(|_| ());
    if pump_outcome.is_err() {
        reconnect_flow(shared);
        return;
    }
    drain_queue(shared);
    let tick = shared.bot.lock().unwrap().world.ticks;
    if tick.saturating_sub(shared.inner.lock().unwrap().last_heartbeat_tick) >= HEARTBEAT_TICKS {
        write_heartbeat(shared);
    }
    idle_watch(shared);
}

fn idle_watch(shared: &Arc<Shared>) {
    // shortcut: the greet/survive host handling needs SDK dialog helpers
    // that are not ported, so the idle watch only reports teleport and
    // cube events for the daemon's rescues.
    let tick = current_ticks(shared);
    {
        let inner = shared.inner.lock().unwrap();
        if inner.script_alive || !inner.connected {
            return;
        }
        if matches!(inner.status.as_str(), "busy" | "reconnecting" | "disconnected" | "starting") {
            return;
        }
        if tick.saturating_sub(inner.last_watch_tick) < 50 {
            return;
        }
    }
    shared.inner.lock().unwrap().last_watch_tick = tick;
    let event = (|| {
        let bot = shared.bot.lock().unwrap();
        if let Some(kind) = randomevent::event_at(&bot) {
            return Some(kind);
        }
        if randomevent::in_fail_teleport(&bot) {
            return Some(randomevent::EventKind::Tower);
        }
        if randomevent::cube_in_pack(&bot) {
            return Some(randomevent::EventKind::Cube);
        }
        None
    })();
    if let Some(kind) = event {
        let line = {
            let bot = shared.bot.lock().unwrap();
            randomevent::report(&bot, kind, false)
        };
        worker_out(&line);
    }
}

fn serve_client(shared: Arc<Shared>, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut line = String::new();
    let outcome = (|| {
        BufReader::new(&stream).read_line(&mut line).map_err(|error| error.to_string())?;
        json::parse(&line).map_err(|error| error.to_string())
    })();
    let reply = match outcome {
        Ok(request) => dispatch(&shared, &request),
        Err(error) => object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("{error:?}")))]),
    };
    let mut buf = json::stringify(&reply);
    buf.push('\n');
    let _ = (&stream).write_all(buf.as_bytes());
}

fn serve(shared: &Arc<Shared>, listener: UnixListener) {
    listener.set_nonblocking(true).ok();
    while !shared.inner.lock().unwrap().closing {
        match listener.accept() {
            Ok((stream, _)) => {
                let worker = Arc::clone(shared);
                std::thread::spawn(move || serve_client(worker, stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => worker_out(&format!("accept failed: {error}")),
        }
        idle_pump(shared);
        std::thread::sleep(Duration::from_millis(IDLE_POLL_MS));
    }
}

// -- entry ------------------------------------------------------------------

fn describe_open_error(error: &str) -> String {
    if error.contains("login") {
        format!("login failed: {error}")
    } else {
        format!("unreachable: {error}")
    }
}

/// Serve one persistent session on `run/session-<account>.sock`.
/// Returns the process exit code.
pub fn run_session(account: &str) -> i32 {
    if account.is_empty() {
        eprintln!("usage: orchestrator session <account>");
        return 2;
    }
    paths::ensure_dirs();
    let (_merged, partial, log_path) = paths::user_paths(account);
    let state_path = std::env::var("LIFESCAPE_STATE").ok().filter(|path| !path.is_empty()).map(PathBuf::from).unwrap_or(partial);
    let (sock_path, ready_path) = paths::session_paths(account);
    let _ = std::fs::remove_file(&sock_path);
    let listener = match UnixListener::bind(&sock_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("bind failed: {error}");
            return 1;
        }
    };
    let no_login = std::env::var("LIFESCAPE_NO_LOGIN").as_deref() == Ok("1");
    let engine = engine_dir();
    let mut bot = Bot::new(&server_host(), server_port(), &format!("{engine}/data/pack"), &format!("{engine}/data/config/private.pem"));
    lifescape_scripts::tables_live::attach_live_tables(&mut bot);
    let shared = Arc::new(Shared {
        account: account.to_string(),
        log_path: log_path.clone(),
        state_path,
        inner: Mutex::new(SessionInner {
            phase: "idle".to_string(),
            script: None,
            script_args: Vec::new(),
            paused: false,
            connected: false,
            status: "starting".to_string(),
            last_error: None,
            last_event: None,
            last_code: None,
            closing: false,
            script_alive: false,
            last_watch_tick: 0,
            last_heartbeat_tick: 0,
            last_msg_logged: 0,
        }),
        bot: Mutex::new(bot),
        heartbeat: Mutex::new(None),
        queue: Mutex::new(VecDeque::new()),
        stop_requested: AtomicBool::new(false),
    });
    {
        let hook_shared = Arc::clone(&shared);
        shared.bot.lock().unwrap().on_pump = Some(std::sync::Arc::new(move |bot| publish_tick(&hook_shared, bot)));
    }
    shared.log(&format!("=== session open {account} ==="));
    if let Ok(meta) = std::fs::metadata(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("?"))) {
        if let Ok(modified) = meta.modified() {
            shared.log(&format!("worker binary built {:?}", modified));
        }
    }
    if !no_login {
        let login_outcome = shared.bot.lock().unwrap().login(account, None).map_err(|error| error.to_string());
        match login_outcome {
            Ok(()) => {
                shared.bot.lock().unwrap().intend("connected");
                shared.inner.lock().unwrap().connected = true;
            }
            Err(error) => {
                let _ = std::fs::write(&ready_path, json::stringify(&object(vec![("ok", Value::Bool(false)), ("error", str_value(&describe_open_error(&error)))])));
                shared.bot.lock().unwrap().close();
                return 1;
            }
        }
    }
    // Static maps load after login: Bot::login resets the world, so any
    // earlier load would be wiped.
    // Map index only (fast): squares paint on demand as the bot moves.
    // Loaded before login so the ready snapshot already covers spawn.
    match crate::staticmap::build_pack(&engine) {
        Ok(pack) => {
            let squares = pack.square_count();
            shared.bot.lock().unwrap().maps = Some(pack);
            shared.log(&format!("map index ready: {squares} squares"));
        }
        Err(error) => shared.log(&format!("static locs missing ({error}); live locs and straight legs only")),
    }
    match crate::staticmap::load_transitions() {
        Ok(rows) => {
            let count = rows.len();
            shared.bot.lock().unwrap().transitions = rows;
            shared.log(&format!("transitions loaded: {count}"));
        }
        Err(error) => shared.log(&format!("transitions missing ({error}); ladders by nearest loc only")),
    }
    shared.inner.lock().unwrap().status = "idle".to_string();
    write_heartbeat(&shared);
    let _ = std::fs::write(&ready_path, json::stringify(&object(vec![("ok", Value::Bool(true)), ("error", str_value(""))])));
    serve(&shared, listener);
    let _ = std::fs::remove_file(&sock_path);
    0
}

