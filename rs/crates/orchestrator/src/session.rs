#![forbid(unsafe_code)]
//! Persistent game session: one connected account, many scripts over time.
//!
//! One connected account over the same unix-socket
//! JSON-lines protocol (ping/status/methods/run/unload/pause/resume/call/
//! close), the same `describe` keys (`ok`/`error`/`session`), the same
//! `run/session-<account>.sock` + `.ready` paths and env (`LIFESCAPE_USER`,
//! `LIFESCAPE_STATE`, `LIFESCAPE_NO_LOGIN`), and the same log markers
//! (`=== session ... ===`, `[HH:MM:SS] [t=<tick> <zone>]` stamps on worker
//! lines). The worker also PUSHES snapshots + events to the daemon socket
//! (`session_push` frames); no `@@EVENT` control lines, no heartbeat file
//! writes in the hot loop (only a last breath at close).
//!
//! Shape of the port: the script runs on one dedicated thread that owns the
//! pump while it lives; the main thread serves the socket and pumps only
//! when idle. Reads come from the last heartbeat snapshot, never from the
//! live bot off its thread. Writes run on the idle pump thread (the
//! safe-point); while a script holds the bot they return `busy` (500-tick
//! bound on each wait).
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

use lifescape_json::{self as json, Value, object, str_value};
use crate::paths;
use crate::session_heartbeat::heartbeat_value;
use crate::session_rescue::PendingRescue;
use crate::zones;

/// Push throttle, in ticks (same pacing as the old heartbeat writer).
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
/// session log): stamped with wall clock and game tick.
/// Script `println!` output bypasses this and stays verbatim.
pub(crate) fn worker_out(shared: &Shared, line: &str) {
    for chunk in line.split('\n') {
        if chunk.is_empty() {
            println!();
        } else {
            println!("{}", shared.stamp_now(chunk));
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
pub(crate) struct QueuedCall {
    pub(crate) method: String,
    pub(crate) args: Vec<Value>,
    pub(crate) started: AtomicBool,
    pub(crate) done: AtomicBool,
    pub(crate) outcome: Mutex<Option<Result<Value, String>>>,
}

pub(crate) struct SessionInner {
    pub(crate) phase: String,
    pub(crate) script: Option<String>,
    pub(crate) script_args: Vec<String>,
    pub(crate) paused: bool,
    /// Manual hold: the idle drain runs no rescue while set (see `hold`).
    pub(crate) held: bool,
    pub(crate) connected: bool,
    pub(crate) status: String,
    pub(crate) last_error: Option<Value>,
    pub(crate) last_event: Option<Value>,
    pub(crate) last_code: Option<Value>,
    pub(crate) closing: bool,
    pub(crate) script_alive: bool,
    pub(crate) last_heartbeat_tick: u64,
    pub(crate) last_msg_logged: usize,
    /// Zone of the last heartbeat, for stamps when the bot is borrowed.
    /// "?" when unknown: explicit, never a silent fallback.
    pub(crate) last_zone: String,
    /// Every tile the bot stood on (x, z, tick), pushed on each pump.
    /// Lets tests check the walker stays on its planned legs; capped,
    /// oldest half dropped.
    pub(crate) trail: Vec<(i32, i32, u64)>,
}

pub(crate) struct Shared {
    pub(crate) account: String,
    pub(crate) log_path: PathBuf,
    pub(crate) state_path: PathBuf,
    pub(crate) inner: Mutex<SessionInner>,
    pub(crate) bot: Mutex<Bot>,
    pub(crate) heartbeat: Mutex<Option<Snapshot>>,
    pub(crate) queue: Mutex<VecDeque<Arc<QueuedCall>>>,
    pub(crate) stop_requested: AtomicBool,
    /// Posed random-event rescue: the pause gate (script thread) or the
    /// idle drain (serve thread) runs it, never the daemon.
    pub(crate) pending_rescue: Mutex<Option<PendingRescue>>,
    /// Single level: set while a rescue runs, its inner pumps skip the gate.
    pub(crate) rescue_gate: AtomicBool,
    /// Last finished rescue + end position: no repeat at the same spot.
    pub(crate) rescue_done: Mutex<Option<(String, (i32, i32, i32))>>,
}

impl Shared {
    fn session_pid() -> u32 {
        std::process::id()
    }

    /// Framework stamp `[HH:MM:SS] [t=<tick> <zone>]`: the exact tick and
    /// zone when the bot lock is free (zone from the pump hook refresh),
    /// else the heartbeat tick + zone (refreshed every ten ticks by the
    /// pump hook), `[t=? ?]` before the first connect.
    fn stamp_now(&self, line: &str) -> String {
        let inner = self.inner.lock().unwrap();
        let (tick_text, zone_text) = if !inner.connected {
            ("?".to_string(), "?".to_string())
        } else {
            match self.bot.try_lock() {
                Ok(bot) => (bot.world.ticks.to_string(), bot.zone_label.clone()),
                Err(_) if inner.last_heartbeat_tick > 0 => (inner.last_heartbeat_tick.to_string(), inner.last_zone.clone()),
                Err(_) => ("?".to_string(), "?".to_string()),
            }
        };
        format!("[{}] [t={tick_text} {zone_text}] {line}", clock_hhmmss())
    }

    pub(crate) fn log(&self, line: &str) {
        append_log(&self.log_path, &self.stamp_now(line));
    }

    pub(crate) fn describe(&self) -> Value {
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
    let zone = if bot.zone_label.is_empty() { "?".to_string() } else { bot.zone_label.clone() };
    // Game lines print after the lock drops: stamping takes the same lock.
    let (pending, trail): (Vec<(String, u64)>, Vec<(i32, i32, u64)>) = {
        let mut inner = shared.inner.lock().unwrap();
        inner.last_zone = zone.clone();
        if bot.world.messages.len() < inner.last_msg_logged {
            inner.last_msg_logged = 0;
        }
        let mut pending = Vec::new();
        while inner.last_msg_logged < bot.world.messages.len() {
            let index = inner.last_msg_logged;
            let text = bot.world.messages.get(index).cloned().unwrap_or_default();
            let at = bot.world.message_ticks.get(index).copied().unwrap_or(tick);
            pending.push((text, at));
            inner.last_msg_logged = index + 1;
        }
        if inner.trail.last().is_none_or(|last| (last.0, last.1) != (bot.world.x, bot.world.z)) {
            inner.trail.push((bot.world.x, bot.world.z, tick));
            if inner.trail.len() > 4000 {
                inner.trail.drain(..2000);
            }
        }
        (pending, inner.trail.clone())
    };
    for (text, at) in &pending {
        worker_out(shared, &format!("msg [t={at} {zone}] {text}"));
    }
    let last = shared.inner.lock().unwrap().last_heartbeat_tick;
    if tick.saturating_sub(last) >= HEARTBEAT_TICKS {
        let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
        let state = heartbeat_value(bot, &shared.account, &script, &trail);
        *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
        shared.inner.lock().unwrap().last_heartbeat_tick = tick;
        crate::session_push::push_snapshot(&shared.account, &state);
    }
    !shared.stop_requested.load(Ordering::SeqCst)
}

/// Atomic heartbeat for `run/users/<account>.bot.json`: the same keys the
/// the heartbeat writer publishes, so `Job::describe` keeps working.
pub(crate) fn write_heartbeat(shared: &Arc<Shared>) {
    let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
    let Ok(bot) = shared.bot.try_lock() else { return };
    let trail = shared.inner.lock().unwrap().trail.clone();
    let state = heartbeat_value(&bot, &shared.account, &script, &trail);
    *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
    let tick = bot.world.ticks;
    let zone = if bot.zone_label.is_empty() { "?".to_string() } else { bot.zone_label.clone() };
    drop(bot);
    shared.inner.lock().unwrap().last_heartbeat_tick = tick;
    shared.inner.lock().unwrap().last_zone = zone;
    crate::session_push::push_snapshot(&shared.account, &state);
}

/// Last breath: the only disk write left. The daemon already serves the
/// pushed snapshots from memory; this file is for daemon-restart adoption.
fn write_last_breath(shared: &Arc<Shared>) {
    let Ok(bot) = shared.bot.try_lock() else { return };
    let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
    let trail = shared.inner.lock().unwrap().trail.clone();
    let state = heartbeat_value(&bot, &shared.account, &script, &trail);
    let _ = paths::write_json(&shared.state_path, &state);
}


// -- commands --------------------------------------------------------------


/// Spawn a script run: the thread owns the bot for its whole life, the
/// socket thread only reads the heartbeat snapshot meanwhile.
pub(crate) fn spawn_script_thread(shared: &Arc<Shared>, script: &str, args: &[String]) {
    shared.stop_requested.store(false, Ordering::SeqCst);
    std::env::set_var("LIFESCAPE_SCRIPT", script);
    shared.log(&format!("=== session run {} {script} ===", shared.account));
    let worker = Arc::clone(shared);
    let script_name = script.to_string();
    let script_args = args.to_vec();
    std::thread::spawn(move || run_script_thread(&worker, &script_name, &script_args));
}

/// Restore the running books for a script resumed after reconnect: the
/// spawn below only starts the thread, these fields are what `run` guards
/// (`busy`), heartbeats and `LIFESCAPE_SCRIPT` read. Without them the
/// resumed run looks idle and a second `run` double-starts the script.
pub(crate) fn mark_resumed(inner: &mut SessionInner, name: &str, args: &[String]) {
    inner.script = Some(name.to_string());
    inner.script_args = args.to_vec();
    inner.phase = "running".to_string();
    inner.status = "running".to_string();
    inner.script_alive = true;
}
pub(crate) fn script_error_type(error: &lifescape_scripts::ScriptError) -> &'static str {
    match error {
        lifescape_scripts::ScriptError::Sdk(_) => "Sdk",
        lifescape_scripts::ScriptError::Usage(_) => "Usage",
    }
}

pub(crate) fn run_script_thread(shared: &Arc<Shared>, script: &str, args: &[String]) {
    let is_rescue = script.starts_with("rescue_");
    if is_rescue {
        // Single level: pumps inside this rescue skip the pause gate.
        shared.rescue_gate.store(true, Ordering::SeqCst);
    }
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
                    worker_out(shared, &format!("{script} interrupted at {:?}", bot.position()));
                    return ("idle".to_string(), None, None);
                }
                let (level, pos_x, pos_z) = bot.position();
                let tick = bot.world.ticks;
                let message = script_error.to_string();
                let error_type = script_error_type(&script_error);
                // Structured push to the daemon; the log keeps one human line.
                crate::session_push::push_event(
                    &shared.account,
                    "script_error",
                    script,
                    false,
                    tick,
                    script,
                    &shared.inner.lock().unwrap().script_args.clone(),
                    &message,
                    (level, pos_x, pos_z),
                );
                shared.inner.lock().unwrap().last_heartbeat_tick = tick;
                shared.log(&format!("event script_error {script} resolved=false"));
                let error = object(vec![
                    ("where", str_value(script)),
                    ("type", str_value(error_type)),
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
        // Keep what was running: a reconnected session resumes it instead
        // of stranding the job idle (stateless scripts restart cleanly
        // from anywhere). Only on operator stop does it stay down.
        let resume = {
            let inner = shared.inner.lock().unwrap();
            match (inner.script.clone(), inner.script_args.clone()) {
                (Some(name), args) if !name.is_empty() => Some((name, args)),
                _ => None,
            }
        };
        reconnect_flow(shared);
        if is_rescue {
            crate::session_rescue::finish_rescue(shared, script);
        }
        // Resume only on a live session with no operator stop: the books
        // go back to running BEFORE the spawn, so guards (`busy`),
        // heartbeats and `LIFESCAPE_SCRIPT` stay consistent and no
        // second `run` can double-start the script.
        if let Some((name, args)) = resume {
            let launch = {
                let inner = shared.inner.lock().unwrap();
                inner.status == "idle" && inner.connected && !shared.stop_requested.load(Ordering::SeqCst) && !inner.closing
            };
            if launch {
                mark_resumed(&mut shared.inner.lock().unwrap(), &name, &args);
                shared.log(&format!("resuming {name} after reconnect"));
                crate::session::spawn_script_thread(shared, &name, &args);
                write_heartbeat(shared);
                return;
            }
        }
        {
            let mut inner = shared.inner.lock().unwrap();
            inner.script_alive = false;
            inner.phase = "idle".to_string();
            inner.script = None;
            inner.script_args = Vec::new();
            if inner.status == "reconnecting" {
                inner.status = "idle".to_string();
            }
            std::env::set_var("LIFESCAPE_SCRIPT", "");
        }
        write_heartbeat(shared);
        return;
    }
    // The failure marker logs after the lock drops: stamping takes it.
    let failed_line: Option<String> = {
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
                None
            }
            "failed" => {
                inner.status = "failed".to_string();
                inner.last_code = outcome.1;
                inner.last_error = outcome.2.clone();
                outcome.2.map(|error| format!("=== session failed {} {} ===", shared.account, json::stringify(&error)))
            }
            _ => {
                inner.status = "idle".to_string();
                if outcome.2.is_some() {
                    inner.last_error = outcome.2;
                }
                None
            }
        }
    };
    if let Some(line) = failed_line {
        shared.log(&line);
    }
    if is_rescue {
        crate::session_rescue::finish_rescue(shared, script);
    }
    shared.stop_requested.store(false, Ordering::SeqCst);
    write_heartbeat(shared);
}


// -- network ---------------------------------------------------------------

pub(crate) fn reconnect_flow(shared: &Arc<Shared>) {
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
            {
                let mut inner = shared.inner.lock().unwrap();
                inner.status = "disconnected".to_string();
                inner.last_error = Some(object(vec![
                    ("where", str_value("relogin")),
                    ("type", str_value("relogin")),
                    ("message", str_value(&error)),
                    ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                ]));
            }
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
    crate::session_cmds::drain_queue(shared);
    if crate::session_rescue::drain_idle(shared) {
        return;
    }
    if shared.inner.lock().unwrap().script_alive {
        return;
    }
    let tick = shared.bot.lock().unwrap().world.ticks;
    if tick.saturating_sub(shared.inner.lock().unwrap().last_heartbeat_tick) >= HEARTBEAT_TICKS {
        write_heartbeat(shared);
    }
    idle_watch(shared);
}

fn idle_watch(shared: &Arc<Shared>) {
    crate::session_rescue::idle_scan(shared);
}

fn serve_client(shared: Arc<Shared>, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut line = String::new();
    let outcome = (|| {
        BufReader::new(&stream).read_line(&mut line).map_err(|error| error.to_string())?;
        json::parse(&line).map_err(|error| error.to_string())
    })();
    let reply = match outcome {
        Ok(request) => crate::session_cmds::dispatch(&shared, &request),
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
            Err(error) => worker_out(shared, &format!("accept failed: {error}")),
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
            held: false,
            connected: false,
            status: "starting".to_string(),
            last_error: None,
            last_event: None,
            last_code: None,
            closing: false,
            script_alive: false,
            last_heartbeat_tick: 0,
            last_msg_logged: 0,
            last_zone: "?".to_string(),
            trail: Vec::new(),
        }),
        bot: Mutex::new(bot),
        heartbeat: Mutex::new(None),
        queue: Mutex::new(VecDeque::new()),
        stop_requested: AtomicBool::new(false),
        pending_rescue: Mutex::new(None),
        rescue_gate: AtomicBool::new(false),
        rescue_done: Mutex::new(None),
    });
    {
        let hook_shared = Arc::clone(&shared);
        shared.bot.lock().unwrap().on_pump = Some(std::sync::Arc::new(move |bot: &mut lifescape_sdk::Bot| {
            bot.zone_label = zones::zone_name(bot.world.x, bot.world.z).map(|(name, _)| name).unwrap_or_else(|| "?".to_string());
            publish_tick(&hook_shared, bot)
        }));
    }
    {
        // Transparent rescue pause: at the end of every pump, a posed
        // event runs its rescue inline on the script thread and bot.
        let gate_shared = Arc::clone(&shared);
        shared.bot.lock().unwrap().pause_gate = Some(std::sync::Arc::new(move |bot| crate::session_rescue::gate_tick(&gate_shared, bot)));
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
    // Planner world and tables for travel_to: static collision plus the
    // transition dump, verdicts and door rules. Missing pieces fail loud
    // at travel time, never silently.
    {
        let mut bot = shared.bot.lock().unwrap();
        let data_dir = crate::paths::root().join("gamedata").join("data");
        let engine_path = std::path::Path::new(&engine);
        let zip = engine_path.join("data").join("pack").join(".cache").join("maps-server.zip");
        match crate::staticmap::load_loctypes(&data_dir.join("gametypes.json")) {
            Ok(defs) => match lifescape_nav::static_world::StaticWorld::open(&zip, defs) {
                Ok(world) => {
                    let content = engine_path.join("../content");
                    match lifescape_nav::f2p::load_zones(&content.join("maps").join("free2play.csv")) {
                        Ok(zones) => world.set_f2p(zones),
                        Err(error) => shared.log(&format!("f2p bounds missing ({error}); travel plans everywhere")),
                    }
                    bot.nav_world = Some(world);
                    shared.log("static world ready for travel_to");
                }
                Err(error) => shared.log(&format!("static world missing ({error}); travel_to refuses")),
            },
            Err(error) => shared.log(&format!("loctypes missing ({error}); travel_to refuses")),
        }
        match lifescape_nav::tables::Tables::load(&data_dir) {
            Ok(tables) => {
                shared.log(&format!("planner tables ready: {} transitions", tables.transitions.len()));
                bot.nav_tables = Some(Arc::new(tables));
            }
            Err(error) => shared.log(&format!("planner tables missing ({error}); travel_to refuses")),
        }
    }
    shared.inner.lock().unwrap().status = "idle".to_string();
    write_heartbeat(&shared);
    let _ = std::fs::write(&ready_path, json::stringify(&object(vec![("ok", Value::Bool(true)), ("error", str_value(""))])));
    {
        // Event observer for the whole session life, idle and running:
        // poses `pending_rescue` for the pause gate / idle drain.
        let watcher = Arc::clone(&shared);
        std::thread::spawn(move || crate::session_rescue::observe_events(&watcher));
    }
    serve(&shared, listener);
    write_last_breath(&shared);
    let _ = std::fs::remove_file(&sock_path);
    0
}

#[cfg(test)]
mod tests {
    use super::{mark_resumed, SessionInner};

    fn idle_inner() -> SessionInner {
        SessionInner {
            phase: "idle".to_string(),
            script: None,
            script_args: Vec::new(),
            paused: false,
            held: false,
            connected: true,
            status: "idle".to_string(),
            last_error: None,
            last_event: None,
            last_code: None,
            closing: false,
            script_alive: false,
            last_heartbeat_tick: 0,
            last_msg_logged: 0,
            last_zone: "?".to_string(),
            trail: Vec::new(),
        }
    }

    /// Resume restores the full running books: a second `run` sees `busy`
    /// (no double-run), heartbeats name the script, the phase reads running.
    #[test]
    fn resume_restores_running_books() {
        let mut inner = idle_inner();
        mark_resumed(&mut inner, "woodcut", &["1".to_string()]);
        assert_eq!(inner.script.as_deref(), Some("woodcut"));
        assert_eq!(inner.script_args, vec!["1".to_string()]);
        assert_eq!(inner.phase, "running");
        assert_eq!(inner.status, "running");
        assert!(inner.script_alive, "run guard sees busy, no double-start");
    }
}

