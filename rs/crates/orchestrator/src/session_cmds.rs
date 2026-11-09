//! Socket commands served by a session worker: run/unload/pause/
//! hold/resume/call/close plus the status/methods dispatch. Split from
//! `session` (file-size rule); state and the socket loop stay there.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

use crate::session::{now_secs, reconnect_flow, spawn_script_thread, worker_out, write_heartbeat, QueuedCall, Shared};
use crate::session_calls::{execute_write, method_lists, read_from_snapshot};

/// A queued SDK call waits at most this long (in ticks) for a safe-point.
const SAFE_POINT_TICKS: u64 = 500;
/// An unload waits this long (in ticks) for the script thread to notice.
const UNLOAD_TICKS: u64 = 250;
/// Process guard when ticks are frozen (offline/CPU spin), in seconds.
const WALL_BACKSTOP: f64 = 30.0;

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
        inner.trail.clear();
    }
    shared.stop_requested.store(false, Ordering::SeqCst);
    shared.budget_stop.store(false, Ordering::SeqCst);
    shared.rescue_done.lock().unwrap().take();
    // Start carrier for the task report (contract scripts only), with a
    // fresh bank baseline: the first bank-open in-run freezes pre-stock.
    *shared.bank_baseline.lock().unwrap() = None;
    *shared.task_start.lock().unwrap() =
        crate::session_report::task_start_for(&shared.bot.lock().unwrap(), script, &string_args);
    spawn_script_thread(shared, script, &string_args);
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
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
            shared.task_start.lock().unwrap().take();
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
        worker_out(shared, "script thread survived unload, it finishes on its own");
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

/// Manual hold: stop the run and let the idle drain pose no rescue until
/// resumed. The session stays open for manual `call` piloting.
fn cmd_hold(shared: &Arc<Shared>) -> Value {
    shared.inner.lock().unwrap().held = true;
    shared.log(&format!("=== session hold {} ===", shared.account));
    cmd_unload(shared)
}

fn cmd_resume(shared: &Arc<Shared>) -> Value {
    {
        let mut inner = shared.inner.lock().unwrap();
        inner.held = false;
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

pub(crate) fn drain_queue(shared: &Arc<Shared>) {
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

/// Posed market delivery: runs inline at the next safe point when a
/// script holds the bot, as its own thread when idle. One convocation at
/// a time (replaces any earlier pose), rescue keeps priority in the gate.
fn cmd_pose_market(shared: &Arc<Shared>, script: &str, args: &[Value]) -> Value {
    let known: Vec<&str> = lifescape_scripts::names();
    if !known.iter().any(|name| *name == script) {
        return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("unknown script {script:?}")))]);
    }
    let string_args: Vec<String> = args
        .iter()
        .map(|arg| match arg {
            Value::Str(text) => text.clone(),
            other => json::stringify(other),
        })
        .collect();
    let tick = current_ticks(shared);
    crate::session_rescue::pose_market(shared, script, &string_args, tick);
    object(vec![("ok", Value::Bool(true)), ("session", shared.describe())])
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
    let thread_name = format!("session-close-{}", worker.account);
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            worker.inner.lock().unwrap().closing = true;
        })
        .expect("session close thread spawns");
    object(vec![("ok", Value::Bool(true)), ("session", session)])
}

pub(crate) fn dispatch(shared: &Arc<Shared>, request: &Value) -> Value {
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
        "hold" => cmd_hold(shared),
        "resume" => cmd_resume(shared),
        "pose_market" => cmd_pose_market(
            shared,
            request.get("script").as_str().unwrap_or(""),
            &request.get("args").as_arr().cloned().unwrap_or_default(),
        ),
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
