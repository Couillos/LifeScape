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
//! Cooperative stop: `stop_requested` → the `on_pump` hook returns false → the
//! SDK raises `Error::Interrupted`, propagated by the pump loops via
//! `is_interrupted`. Budget ends (engagement ticks, unload/close) are
//! requested through that flag and deferred to the next safe point by the
//! pause gate; a running walk mid-leg finishes its pump, it is never frozen.

use std::collections::{HashMap, VecDeque};
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
use crate::session_report::{report_value, snapshot_holdings, ReportCtx, TaskStart};
use crate::session_rescue::PendingRescue;
use crate::zones;

/// Push throttle, in ticks (same pacing as the old heartbeat writer).
const HEARTBEAT_TICKS: u64 = 10;
/// Wall-clock twin of the tick throttle: a long plan freezes ticks
/// without freezing the worker, so one snapshot per second still flows
/// with the true (frozen) tick instead of a dead frame.
const HEARTBEAT_WALL_SECS: f64 = 1.0;
/// Idle pump cadence when active (E5); at rest the serve loop sleeps
/// IDLE_REST_MS instead: 1 wakeup/s, not 10.
const IDLE_POLL_MS: u64 = 100;
const IDLE_REST_MS: u64 = 1000;

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
    // E11 buffered sink (dédup + rotation 100 Mo): no open/write/close
    // per line anymore; heartbeats and close flush.
    crate::session_log::append_log(log_path, line);
}

pub(crate) fn engine_dir() -> String {
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
    /// Wall time of the last pushed heartbeat: the tick gate alone
    /// starves the pipe while a long plan freezes ticks.
    pub(crate) last_heartbeat_wall: f64,
    /// Last FULL snapshot pushed (E4 two-flow): tick, wall, and the
    /// script/status it carried. Light frames update the pair above.
    pub(crate) last_full_tick: u64,
    pub(crate) last_full_wall: f64,
    pub(crate) last_full_script: String,
    pub(crate) last_full_status: String,
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
    /// Set by the pump hook when the engagement budget ends at a safe
    /// point (not by the operator): maps Interrupted to budget_exhausted.
    pub(crate) budget_stop: AtomicBool,
    /// Run start carrier for the task report (skill scripts only).
    pub(crate) task_start: Mutex<Option<TaskStart>>,
    /// Last seen bank contents (bank streams only while open): the end
    /// snapshot reuses it, so internal trips land in the diff.
    pub(crate) last_bank: Mutex<HashMap<u16, u32>>,
    /// Bank contents at first bank-open of the current run: pre-stock
    /// cancels against it instead of popping as phantom gains.
    pub(crate) bank_baseline: Mutex<Option<HashMap<u16, u32>>>,
    /// Market-pause ticks billed to the current engagement (hook sums
    /// them; the report diffs start/end).
    pub(crate) market_pause_ticks: Mutex<u64>,
    /// Posed market delivery: the pause gate (script thread) or the idle
    /// drain (serve thread) runs it, never the daemon.
    pub(crate) pending_market: Mutex<Option<crate::session_rescue::PendingMarket>>,
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
            // Hold survives a daemon restart: `restore_jobs` reads it back.
            ("held", Value::Bool(inner.held)),
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

/// Due when the route moved (ticks) or the worker proved alive (wall):
/// a long plan freezes ticks without freezing the worker, and the old
/// tick-only gate left spectate showing a dead frame as live. Same tick,
/// fresh `updated`: honest, never faked progress.
pub(crate) fn heartbeat_due(last_tick: u64, tick: u64, last_wall: f64, now: f64) -> bool {
    tick.saturating_sub(last_tick) >= HEARTBEAT_TICKS || now - last_wall >= HEARTBEAT_WALL_SECS
}

/// Throttled heartbeat for a bot borrowed by the script thread: same
/// payload as `write_heartbeat`, no bot lock needed. Installed as the
/// bot's pump hook so spectate/route/calls stay live during runs.
/// Returns false once the operator stops the run (unload/close): the pump
/// aborts and the script unwinds instead of finishing its action.
fn publish_tick(shared: &Shared, bot: &lifescape_sdk::Bot) -> bool {    let tick = bot.world.ticks;
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
    let last_wall = shared.inner.lock().unwrap().last_heartbeat_wall;
    if let Some(live) = crate::session_report::refresh_bank_cache(bot, &mut shared.last_bank.lock().unwrap()) {
        // First bank-open of a run freezes the pre-stock baseline; every
        // later open only refreshes the end cache.
        if shared.task_start.lock().unwrap().is_some() && shared.bank_baseline.lock().unwrap().is_none() {
            *shared.bank_baseline.lock().unwrap() = Some(live);
        }
    }
    if heartbeat_due(last, tick, last_wall, now_secs()) {
        crate::heartbeat_flows::pump_heartbeat(shared, bot, &trail);
    }
    if shared.stop_requested.load(Ordering::SeqCst) {
        return false;
    }
    // Engagement end (hook backstop for non-cycle scripts): deferred to
    // the safe point by `budget_due`, flagged so the run end reports
    // budget_exhausted instead of stopped.
    if budget_due(shared, bot) {
        shared.budget_stop.store(true, Ordering::SeqCst);
        return false;
    }
    true
}

/// Atomic heartbeat for `run/users/<account>.bot.json`: the same keys the
/// the heartbeat writer publishes, so `Job::describe` keeps working.
/// Full flow (E4): explicit state changes flush here; the periodic idle
/// path prefers the light flow. Always flushes the E11 log buffer: at
/// most one heartbeat of lines is ever memory-only.
pub(crate) fn write_heartbeat(shared: &Arc<Shared>) {
    let script = shared.inner.lock().unwrap().script.clone().unwrap_or_default();
    let Ok(bot) = shared.bot.try_lock() else { return };
    let trail = shared.inner.lock().unwrap().trail.clone();
    let state = heartbeat_value(&bot, &shared.account, &script, &trail);
    *shared.heartbeat.lock().unwrap() = Some(bot.snapshot());
    let tick = bot.world.ticks;
    let zone = if bot.zone_label.is_empty() { "?".to_string() } else { bot.zone_label.clone() };
    drop(bot);
    let status_now = shared.inner.lock().unwrap().status.clone();
    shared.inner.lock().unwrap().last_heartbeat_tick = tick;
    shared.inner.lock().unwrap().last_heartbeat_wall = now_secs();
    shared.inner.lock().unwrap().last_zone = zone;
    shared.inner.lock().unwrap().last_full_tick = tick;
    shared.inner.lock().unwrap().last_full_wall = now_secs();
    shared.inner.lock().unwrap().last_full_script = script;
    shared.inner.lock().unwrap().last_full_status = status_now;
    crate::session_push::push_snapshot(&shared.account, &state);
    crate::session_log::flush_log(&shared.log_path);
}

/// Last breath: the only disk write left. The daemon already serves the
/// pushed snapshots from memory; this file is for daemon-restart adoption.
fn write_last_breath(shared: &Arc<Shared>) {
    crate::session_log::flush_log(&shared.log_path);
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
    shared.budget_stop.store(false, Ordering::SeqCst);
    std::env::set_var("LIFESCAPE_SCRIPT", script);
    shared.log(&format!("=== session run {} {script} ===", shared.account));
    let worker = Arc::clone(&shared);
    let script_name = script.to_string();
    let script_args = args.to_vec();
    let thread_name = format!("script-{}", shared.account);
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || run_script_thread(&worker, &script_name, &script_args))
        .expect("script thread spawns");
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
        lifescape_scripts::ScriptError::Failed(_) => "Failed",
        lifescape_scripts::ScriptError::Blocked { .. } => "Blocked",
    }
}

/// Engagement budget due at a safe point: work ticks (wall minus market
/// pauses) past the max, alive with no modal open. The hook aborts the
/// pump with Interrupted; the run end maps it to budget_exhausted.
pub(crate) fn budget_due(shared: &Shared, bot: &lifescape_sdk::Bot) -> bool {
    let start = shared.task_start.lock().unwrap();
    let Some(start) = start.as_ref() else {
        return false;
    };
    if start.budget_max == 0 {
        return false;
    }
    let wall = bot.world.ticks.saturating_sub(start.holdings.tick);
    let work = wall.saturating_sub(bot.pause_ticks.saturating_sub(start.pause_ticks));
    work >= start.budget_max && lifescape_scripts::kit::survival::at_safe_point(bot)
}

pub(crate) fn run_script_thread(shared: &Arc<Shared>, script: &str, args: &[String]) {
    let is_rescue = script.starts_with("rescue_");
    if is_rescue {
        // Single level: pumps inside this rescue skip the pause gate.
        shared.rescue_gate.store(true, Ordering::SeqCst);
    }
    let outcome: (String, Option<Value>, Option<Value>, String, Vec<String>) = (|| {
        let mut bot = shared.bot.lock().unwrap();
        match lifescape_scripts::run(script, &mut bot, args) {
            Ok(code) => {
                if code == 0 {
                    ("finished".to_string(), Some(Value::Int(i64::from(code))), None, "goal_reached".to_string(), Vec::new())
                } else if code == lifescape_scripts::kit::cycle::BUDGET_EXIT {
                    ("finished".to_string(), Some(Value::Int(i64::from(code))), None, "budget_exhausted".to_string(), Vec::new())
                } else {
                    let (level, pos_x, pos_z) = bot.position();
                    let error = object(vec![
                        ("where", str_value(script)),
                        ("type", str_value("exit")),
                        ("message", str_value(&format!("run returned {code}"))),
                        ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                    ]);
                    ("failed".to_string(), Some(Value::Int(i64::from(code))), Some(error), "failed".to_string(), Vec::new())
                }
            }
            Err(script_error) => {
                use lifescape_sdk::error::Error as SdkError;
                let reconnect = matches!(&script_error, lifescape_scripts::ScriptError::Sdk(SdkError::Io(_)) | lifescape_scripts::ScriptError::Sdk(SdkError::Login(_)));
                if reconnect {
                    // The trigger would otherwise vanish: relogin only
                    // logs its own outcome, so a flap loop is undiagnosable.
                    worker_out(shared, &format!("{script} reconnect on {script_error}"));
                    return ("reconnect".to_string(), None, None, String::new(), Vec::new());
                }
                if matches!(&script_error, lifescape_scripts::ScriptError::Sdk(SdkError::Interrupted(_))) {
                    // Operator unload/close: back to idle, no rescue event.
                    // Flagged by the budget hook instead: engagement ended.
                    worker_out(shared, &format!("{script} interrupted at {:?}", bot.position()));
                    let stop = if shared.budget_stop.swap(false, Ordering::SeqCst) { "budget_exhausted" } else { "stopped" };
                    return ("idle".to_string(), None, None, stop.to_string(), Vec::new());
                }
                if let lifescape_scripts::ScriptError::Blocked { missing } = &script_error {
                    let (level, pos_x, pos_z) = bot.position();
                    let message = script_error.to_string();
                    let error = object(vec![
                        ("where", str_value(script)),
                        ("type", str_value("Blocked")),
                        ("message", str_value(&message)),
                        ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                    ]);
                    shared.log(&format!("blocked {script} missing {missing:?}, brain files a sub-task"));
                    let phase = if is_rescue { "failed".to_string() } else { "idle".to_string() };
                    return (phase, None, Some(error), "blocked".to_string(), missing.clone());
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
                shared.log(&format!("event script_error {script} resolved=false {error_type}: {message}"));
                let error = object(vec![
                    ("where", str_value(script)),
                    ("type", str_value(error_type)),
                    ("message", str_value(&message)),
                    ("at", Value::Arr(vec![Value::Int(i64::from(level)), Value::Int(i64::from(pos_x)), Value::Int(i64::from(pos_z))])),
                ]);
                if is_rescue {
                    ("failed".to_string(), None, Some(error), "failed".to_string(), Vec::new())
                } else {
                    ("idle".to_string(), None, Some(error), "failed".to_string(), Vec::new())
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
    // Task report (contract v2): diff the start holdings, push to the
    // daemon, log one JSON line. Rescues and legacy scripts carry no
    // start snapshot and report nothing structured.
    if !is_rescue {
        if let Some(start) = shared.task_start.lock().unwrap().take() {
            let bot = shared.bot.lock().unwrap();
            let end = snapshot_holdings(&bot);
            // The end cache counts only when the bank opened in-run
            // (baseline set): a run that never banks sees no bank moves,
            // so pre-stock stays invisible instead of popping as gains.
            let baseline = shared.bank_baseline.lock().unwrap().take();
            let bank_end = if baseline.is_some() { shared.last_bank.lock().unwrap().clone() } else { HashMap::new() };
            let baseline = baseline.unwrap_or_default();
            let report = crate::session_report::build_report(
                &bot,
                &start.holdings,
                &baseline,
                &end,
                &bank_end,
                &ReportCtx {
                    activity: start.script.clone(),
                    node_id: start.node_id,
                    stop: outcome.3.clone(),
                    missing: outcome.4.clone(),
                    error: outcome.2.as_ref().map(|error| {
                        (
                            error.get("type").as_str().unwrap_or("?").to_string(),
                            error.get("message").as_str().unwrap_or("").to_string(),
                        )
                    }),
                },
            );
            drop(bot);
            let value = report_value(&shared.account, &report);
            crate::session_push::push_task_report(&shared.account, &value);
            shared.log(&format!("task_report {}", json::stringify(&value)));
        }
    }
    shared.budget_stop.store(false, Ordering::SeqCst);
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
    if let Err(error) = &pump_outcome {
        // Same invisibility as the script path: log the trigger, the
        // relogin outcome alone cannot explain a flap loop.
        shared.log(&format!("idle pump failed: {error}"));
    }
    if pump_outcome.is_err() {
        reconnect_flow(shared);
        return;
    }
    crate::session_cmds::drain_queue(shared);
    if crate::session_rescue::drain_idle(shared) {
        return;
    }
    if crate::session_rescue::drain_idle_market(shared) {
        return;
    }
    if shared.inner.lock().unwrap().script_alive {
        return;
    }
    let tick = shared.bot.lock().unwrap().world.ticks;
    let (last, last_wall) = {
        let inner = shared.inner.lock().unwrap();
        (inner.last_heartbeat_tick, inner.last_heartbeat_wall)
    };
    if heartbeat_due(last, tick, last_wall, now_secs()) {
        // E4: periodic idle flow is light; full frames ride explicit
        // state changes (`write_heartbeat`) or the full cadence.
        crate::heartbeat_flows::idle_heartbeat_flow(shared, tick);
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
                let thread_name = format!("session-conn-{}", worker.account);
                std::thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || serve_client(worker, stream))
                    .expect("session connection thread spawns");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => worker_out(shared, &format!("accept failed: {error}")),
        }
        idle_pump(shared);
        // E5 event-driven rest: at rest (no script, no queued call, no
        // posed rescue/market) the serve loop wakes 1/s, not 10/s. With
        // the 1 s observer below, an idle worker wakes <= 2/s.
        let resting = {
            let inner = shared.inner.lock().unwrap();
            !inner.script_alive
                && shared.queue.lock().unwrap().is_empty()
                && shared.pending_rescue.lock().unwrap().is_none()
                && shared.pending_market.lock().unwrap().is_none()
        };
        std::thread::sleep(Duration::from_millis(if resting { IDLE_REST_MS } else { IDLE_POLL_MS }));
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
    crate::session_locks::install(bot.locks.clone());
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
            last_heartbeat_wall: 0.0,
            last_full_tick: 0,
            last_full_wall: 0.0,
            last_full_script: String::new(),
            last_full_status: String::new(),
            last_msg_logged: 0,
            last_zone: "?".to_string(),
            trail: Vec::new(),
        }),
        bot: Mutex::new(bot),
        heartbeat: Mutex::new(None),
        queue: Mutex::new(VecDeque::new()),
        stop_requested: AtomicBool::new(false),
        budget_stop: AtomicBool::new(false),
        task_start: Mutex::new(None),
        last_bank: Mutex::new(HashMap::new()),
        bank_baseline: Mutex::new(None),
        market_pause_ticks: Mutex::new(0),
        pending_market: Mutex::new(None),
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
    // Workers carry no map: navigation is 100 % service-side. The daemon
    // holds the single snapshot; the worker only wires the plan handle
    // and pins the snapshot version. A down or stale service fails loud
    // at travel time (`nav-down`), never with a local plan.
    {
        let mut bot = shared.bot.lock().unwrap();
        let data_dir = crate::paths::root().join("gamedata").join("data");
        let engine_path = std::path::Path::new(&engine);
        let zip = engine_path.join("data").join("pack").join(".cache").join("maps-server.zip");
        let f2p = engine_path.join("../content").join("maps").join("free2play.csv");
        bot.plan_client =
            Some(lifescape_nav::service::PlanClient::new(&crate::plan_service::socket_path(), account));
        bot.plan_data_version = Some(crate::plan_service::version_for(&data_dir, &zip, &f2p));
        shared.log("plan service wired, no local map");
    }
    shared.inner.lock().unwrap().status = "idle".to_string();
    write_heartbeat(&shared);
    let _ = std::fs::write(&ready_path, json::stringify(&object(vec![("ok", Value::Bool(true)), ("error", str_value(""))])));
    {
        // Event observer for the whole session life, idle and running:
        // poses `pending_rescue` for the pause gate / idle drain.
        let watcher = Arc::clone(&shared);
        let thread_name = format!("session-observe-{}", shared.account);
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || crate::session_rescue::observe_events(&watcher))
            .expect("session observer thread spawns");
    }
    serve(&shared, listener);
    write_last_breath(&shared);
    let _ = std::fs::remove_file(&sock_path);
    0
}

#[cfg(test)]
mod tests {
    use super::{heartbeat_due, mark_resumed, SessionInner, Shared};

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
            last_heartbeat_wall: 0.0,
            last_full_tick: 0,
            last_full_wall: 0.0,
            last_full_script: String::new(),
            last_full_status: String::new(),
            last_msg_logged: 0,
            last_zone: "?".to_string(),
            trail: Vec::new(),
        }
    }

    /// The wall twin pushes during tick-frozen plans (same tick, stale
    /// wall); a fresh wall or a short tick run stays quiet.
    #[test]
    fn heartbeat_due_covers_tick_frozen_plans() {
        assert!(heartbeat_due(100, 115, 0.0, 10.0), "ten ticks moved");
        assert!(heartbeat_due(100, 100, 10.0, 13.5), "same tick, stale wall pushes");
        assert!(!heartbeat_due(100, 100, 10.0, 10.5), "same tick, fresh wall stays quiet");
        assert!(!heartbeat_due(100, 109, 10.0, 10.5), "nine ticks are not ten");
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

    fn budget_shared(budget_max: u64, start_tick: u64) -> Shared {
        use std::collections::VecDeque;
        use std::sync::{atomic::AtomicBool, Mutex};
        let mut bot = lifescape_sdk::Bot::new("127.0.0.1", 43594, ".", ".");
        bot.world.ticks = start_tick;
        let mut start_holdings = crate::session_report::snapshot_holdings(&bot);
        start_holdings.tick = start_tick;
        Shared {
            account: "t01".to_string(),
            log_path: std::path::PathBuf::from("/tmp/opencode-budget-test.log"),
            state_path: std::path::PathBuf::from("/tmp/opencode-budget-test.bot.json"),
            inner: Mutex::new(idle_inner()),
            bot: Mutex::new(bot),
            heartbeat: Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            stop_requested: AtomicBool::new(false),
            budget_stop: AtomicBool::new(false),
            task_start: Mutex::new(Some(crate::session_report::TaskStart {
                script: "woodcut".to_string(),
                budget_max,
                node_id: "n9".to_string(),
                holdings: start_holdings,
                pause_ticks: 0,
            })),
            last_bank: Mutex::new(std::collections::HashMap::new()),
            bank_baseline: Mutex::new(None),
            market_pause_ticks: Mutex::new(0),
            pending_market: Mutex::new(None),
            pending_rescue: Mutex::new(None),
            rescue_gate: AtomicBool::new(false),
            rescue_done: Mutex::new(None),
        }
    }

    /// Budget hook: over budget at a safe point aborts (the run end maps
    /// it to budget_exhausted); under budget or off the safe point keeps
    /// pumping. No task start means no hook.
    #[test]
    fn budget_hook_fires_only_over_budget_at_safe_point() {
        let shared = budget_shared(6000, 1000);
        {
            let mut bot = shared.bot.lock().unwrap();
            bot.world.ticks = 6999;
            assert!(!super::budget_due(&shared, &bot), "one tick under keeps pumping");
            bot.world.ticks = 7000;
            assert!(super::budget_due(&shared, &bot), "budget at a safe point aborts");
            bot.world.modal =
                Some(lifescape_sdk::world::Modal { layer: "chat".to_string(), interface_id: 1 });
            assert!(!super::budget_due(&shared, &bot), "open modal defers the abort");
        }
        *shared.task_start.lock().unwrap() = None;
        {
            let bot = shared.bot.lock().unwrap();
            assert!(!super::budget_due(&shared, &bot), "no task start, no hook");
        }
    }
}

