#![forbid(unsafe_code)]
//! Supervision: one thread per account, rescue state machine, event tail.
//! Methods on daemon::Orchestrator; session control stays in daemon.rs.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::daemon::{Orchestrator, WORKER_TIMEOUT};
use crate::jobs::{Job, now_secs};
use crate::json::{self, Value, object, str_value};
use crate::paths;
use crate::registry;

// Random-event geography, decoded from scripts/randomevents.py.
const MAZE_X: (i64, i64) = (2880, 2944);
const MAZE_Z: (i64, i64) = (4544, 4608);
const MIME_X: (i64, i64) = (1984, 2048);
const MIME_Z: (i64, i64) = (4736, 4800);
const FAIL_TELEPORTS: &[(i64, i64, i64)] = &[
    (1, 3278, 3500), (1, 3097, 3268), (2, 2954, 3388), (3, 3221, 3474),
    (1, 3229, 3223), (0, 3097, 9567), (2, 3287, 3190), (1, 2964, 3213),
    (0, 2968, 3335), (0, 2935, 3515), (0, 3212, 3423), (0, 3076, 3412),
    (0, 2996, 3145), (1, 3025, 3251), (1, 2975, 3382), (1, 3214, 3215),
    (1, 3112, 3157), (0, 3246, 3451), (0, 3283, 3218), (0, 3045, 3491),
    (0, 3083, 3469), (2, 3104, 3163), (0, 3218, 9618), (0, 3263, 3424),
];

impl Orchestrator {
    /// One thread per account: accounts are watched in parallel.
    pub(crate) fn start_supervisor(self: &Arc<Self>, job: Arc<Job>) {
        let orch = self.clone();
        std::thread::spawn(move || {
            while orch.running.load(Ordering::SeqCst) && job.inner.lock().unwrap().supervise {
                orch.supervise_once(&job);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }

    fn supervise_once(&self, job: &Arc<Job>) {
        if self.procs.lock().unwrap().contains_key(&job.account) && !self.proc_alive(&job.account) {
            self.handle_worker_death(job);
            return;
        }
        let session = self
            .send_worker(
                &job.account,
                &object(vec![("cmd", str_value("status"))]),
                Duration::from_secs(5),
            )
            .ok()
            .and_then(|reply| {
                if reply.get("session").is_null() {
                    None
                } else {
                    Some(reply.get("session").clone())
                }
            });
        if let Some(ref session) = session {
            job.apply_session(session);
        }
        self.read_events(job);
        self.maybe_rescue_from_worker(job, session.as_ref());
        if let Some(event) = self.detect_state_events(job) {
            self.handle_event(job, &event);
        }
        self.rescue_step(job);
        self.clear_event_if_gone(job);
        self.auto_idle_finished(job);
        let write = {
            let mut inner = job.inner.lock().unwrap();
            if now_secs() - inner.state_written_at >= 2.0 {
                inner.state_written_at = now_secs();
                true
            } else {
                false
            }
        };
        if write {
            self.write_state(job);
        }
    }

    fn read_events(&self, job: &Arc<Job>) {
        let (_merged, _partial, log_path) = paths::user_paths(&job.account);
        let data = (|| {
            let mut handle = std::fs::File::open(&log_path).ok()?;
            use std::io::Seek;
            let mut offset = job.inner.lock().unwrap().log_offset;
            handle.seek(std::io::SeekFrom::Start(offset)).ok()?;
            let mut chunk = Vec::new();
            use std::io::Read;
            handle.read_to_end(&mut chunk).ok()?;
            offset += chunk.len() as u64;
            job.inner.lock().unwrap().log_offset = offset;
            Some(chunk)
        })();
        let Some(chunk) = data else { return };
        let text = String::from_utf8_lossy(&chunk);
        for line in text.split('\n') {
            if !line.starts_with("@@EVENT ") {
                continue;
            }
            if let Ok(event) = json::parse(&line["@@EVENT ".len()..]) {
                self.handle_event(job, &event);
            }
        }
    }

    fn handle_event(&self, job: &Arc<Job>, event: &Value) {
        if event.get("kind").as_str() == Some("quest_requirements") {
            // quest_state completion is not ported: record the event as-is.
            job.inner.lock().unwrap().requirements = Some(event.clone());
            println!(
                "job {}: quest {} requirements met={} ({} reasons)",
                job.account,
                event.get("quest").as_str().unwrap_or(""),
                event.get("met").as_bool().unwrap_or(false),
                event.get("reasons").as_arr().map(|r| r.len()).unwrap_or(0),
            );
            return;
        }
        if event.get("kind").as_str() != Some("random_event") {
            return;
        }
        let name = event.get("event").as_str().unwrap_or("unknown").to_string();
        let resolved = event.get("resolved").as_bool().unwrap_or(false);
        println!("job {}: random event {name} resolved={resolved}", job.account);
        {
            let mut inner = job.inner.lock().unwrap();
            let previous = inner.last_event.clone().unwrap_or(Value::Null);
            let mut entry = std::collections::BTreeMap::new();
            entry.insert("event".to_string(), str_value(&name));
            entry.insert("resolved".to_string(), Value::Bool(resolved));
            entry.insert("at".to_string(), Value::Float(now_secs()));
            for key in ["script", "args"] {
                if !previous.get(key).is_null() {
                    entry.insert(key.to_string(), previous.get(key).clone());
                }
            }
            inner.last_event = Some(Value::Obj(entry));
            if resolved {
                return;
            }
            // Manual hold: record the event for visibility, run nothing.
            if inner.held {
                return;
            }
            if inner.rescue_for.is_some() {
                return;
            }
            if inner.phase == "running" && inner.script.is_some() {
                inner.interrupted = Some((inner.script.clone().unwrap(), inner.script_args.clone()));
                drop(inner);
                let _ = self.send_worker(
                    &job.account,
                    &object(vec![("cmd", str_value("unload"))]),
                    Duration::from_secs(WORKER_TIMEOUT),
                );
            } else {
                // The worker already aborted the script before the daemon
                // read this event: resume from the script retained above.
                let retained = inner.last_event.clone().unwrap_or(Value::Null);
                if !retained.get("script").is_null() {
                    let script = retained.get("script").as_str().unwrap_or("").to_string();
                    let args = retained.get("args").as_arr().cloned().unwrap_or_default();
                    inner.interrupted = Some((script, args));
                }
            }
        }
        let rescue = registry::rescue_for(&name).to_string();
        self.start_rescue(job, &rescue);
    }

    /// Run a rescue script in the session, or record why it failed.
    fn start_rescue(&self, job: &Arc<Job>, rescue: &str) {
        let reply = self.send_worker(
            &job.account,
            &object(vec![
                ("cmd", str_value("run")),
                ("script", str_value(rescue)),
                ("args", Value::Arr(vec![])),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let mut inner = job.inner.lock().unwrap();
        match reply {
            Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
                inner.rescue_for = Some(rescue.to_string());
                inner.phase = "running".to_string();
                inner.script = Some(rescue.to_string());
                inner.script_args = Vec::new();
                inner.status = "running".to_string();
            }
            Ok(reply) => {
                inner.last_error = Some(object(vec![
                    ("where", str_value("rescue")),
                    ("type", str_value("rescue")),
                    ("message", reply.get("error").clone()),
                    ("at", Value::Arr(vec![])),
                ]));
            }
            Err(error) => {
                inner.last_error = Some(object(vec![
                    ("where", str_value("rescue")),
                    ("type", str_value("rescue")),
                    ("message", str_value(&error)),
                    ("at", Value::Arr(vec![])),
                ]));
            }
        }
    }

    fn maybe_rescue_from_worker(&self, job: &Arc<Job>, session: Option<&Value>) {
        let Some(session) = session else { return };
        if job.inner.lock().unwrap().held {
            return;
        }
        {
            let mut inner = job.inner.lock().unwrap();
            if inner.rescue_for.is_some() {
                return;
            }
            let last = session.get("last_event");
            if last.as_obj().is_none() {
                return;
            }
            if last.get("event").as_str() != Some("random_event")
                || last.get("resolved").as_bool().unwrap_or(true)
            {
                return;
            }
            if session.get("phase").as_str() != Some("idle") || !session.get("script").is_null() {
                return;
            }
            if last.get("script").as_str().unwrap_or("").is_empty() {
                return;
            }
            if inner.worker_rescue_at.as_ref() == Some(last.get("at")) {
                return;
            }
            inner.worker_rescue_at = Some(last.get("at").clone());
            inner.interrupted = Some((
                last.get("script").as_str().unwrap_or("").to_string(),
                last.get("args").as_arr().cloned().unwrap_or_default(),
            ));
        }
        self.start_rescue(job, "rescue_fallback");
    }

    /// The rescue ran: drop the daemon event when the live state shows
    /// none (maze/mime rooms, fail tiles, carried cube). Rescues replay
    /// a stale unresolved event every tick otherwise.
    fn clear_event_if_gone(&self, job: &Arc<Job>) {
        let state = {
            let (merged_path, partial_path, _log) = paths::user_paths(&job.account);
            let merged = paths::read_json(&merged_path).unwrap_or(Value::Null);
            match (merged, paths::read_json(&partial_path)) {
                (Value::Obj(mut base), Some(Value::Obj(extra))) => {
                    base.extend(extra);
                    Value::Obj(base)
                }
                (base, _) => base,
            }
        };
        if state.as_obj().is_none() {
            return;
        }
        let fresh = state
            .get("updated")
            .as_f64()
            .is_some_and(|at| now_secs() - at < 10.0);
        if !fresh {
            return;
        }
        let pos = state.get("position").as_arr().cloned().unwrap_or_default();
        let (level, x, z) = (
            pos.first().and_then(|v| v.as_i64()).unwrap_or(-1),
            pos.get(1).and_then(|v| v.as_i64()).unwrap_or(0),
            pos.get(2).and_then(|v| v.as_i64()).unwrap_or(0),
        );
        let mut present = MAZE_X.0 <= x && x < MAZE_X.1 && MAZE_Z.0 <= z && z < MAZE_Z.1
            || MIME_X.0 <= x && x < MIME_X.1 && MIME_Z.0 <= z && z < MIME_Z.1
            || FAIL_TELEPORTS.contains(&(level, x, z));
        if !present {
            let inv = state.get("inv").as_arr().cloned().unwrap_or_default();
            present = inv.iter().any(|entry| {
                entry.get("name").as_str() == Some("macro_cube") || entry.as_str() == Some("macro_cube")
            });
        }
        if present {
            return;
        }
        let mut inner = job.inner.lock().unwrap();
        if let Some(Value::Obj(mut obj)) = inner.last_event.take() {
            if obj.get("resolved").and_then(|v| v.as_bool()) != Some(true) {
                obj.insert("resolved".to_string(), Value::Bool(true));
            }
            inner.last_event = Some(Value::Obj(obj));
        }
    }
    fn detect_state_events(&self, job: &Arc<Job>) -> Option<Value> {
        let script = {
            let inner = job.inner.lock().unwrap();
            if inner.rescue_for.is_some() {
                return None;
            }
            if matches!(
                inner.status.as_str(),
                "failed" | "disconnected" | "reconnecting" | "busy" | "starting" | "stopped"
            ) {
                return None;
            }
            inner.script.clone().unwrap_or_default()
        };
        if script.starts_with("quest_") || script.starts_with("rescue_") {
            return None;
        }
        let (merged_path, partial_path, _log) = paths::user_paths(&job.account);
        let merged = paths::read_json(&merged_path).unwrap_or(Value::Null);
        let state = match (merged, paths::read_json(&partial_path)) {
            (Value::Obj(mut base), Some(Value::Obj(extra))) => {
                base.extend(extra);
                Value::Obj(base)
            }
            (base, _) => base,
        };
        if state.as_obj().is_none() {
            return None;
        }
        let pos = state.get("position").as_arr().cloned().unwrap_or_default();
        let mut event: Option<&str> = None;
        if pos.len() > 2 {
            let (level, x, z) = (
                pos[0].as_i64().unwrap_or(0),
                pos[1].as_i64().unwrap_or(0),
                pos[2].as_i64().unwrap_or(0),
            );
            if MAZE_X.0 <= x && x < MAZE_X.1 && MAZE_Z.0 <= z && z < MAZE_Z.1 {
                event = Some("maze");
            } else if MIME_X.0 <= x && x < MIME_X.1 && MIME_Z.0 <= z && z < MIME_Z.1 {
                event = Some("mime");
            } else if FAIL_TELEPORTS.contains(&(level, x, z)) {
                let last = job.inner.lock().unwrap().event_last_pos;
                if last.is_none() || (x - last.unwrap().0).abs() + (z - last.unwrap().1).abs() > 100
                {
                    event = Some("tower");
                }
            }
            job.inner.lock().unwrap().event_last_pos = Some((x, z));
        }
        if event.is_none() && script.is_empty() {
            let inv = state.get("inv").as_arr().cloned().unwrap_or_default();
            if inv.iter().any(|entry| {
                entry.get("name").as_str() == Some("macro_cube") || entry.as_str() == Some("macro_cube")
            }) {
                event = Some("cube");
            }
        }
        let event = event?;
        {
            let mut inner = job.inner.lock().unwrap();
            if now_secs() - inner.event_detect_at < 60.0 {
                return None;
            }
            inner.event_detect_at = now_secs();
        }
        println!("job {}: state-detected {event} at {}", job.account, json::stringify(state.get("position")));
        Some(object(vec![
            ("kind", str_value("random_event")),
            ("event", str_value(event)),
            ("resolved", Value::Bool(false)),
        ]))
    }

    /// A finished rescue hands the session back; a failed one ends it.
    fn rescue_step(&self, job: &Arc<Job>) {
        let next = {
            let mut inner = job.inner.lock().unwrap();
            if inner.rescue_for.is_none() || inner.phase == "running" {
                return;
            }
            let rescue = inner.rescue_for.clone().unwrap_or_default();
            inner.rescue_for = None;
            if inner.status == "failed" {
                inner.interrupted = None;
                println!("job {}: rescue {rescue} failed, session failed", job.account);
                return;
            }
            inner.interrupted.clone()
        };
        let Some((script, args)) = next else {
            job.inner.lock().unwrap().status = "idle".to_string();
            return;
        };
        job.inner.lock().unwrap().interrupted = None;
        let reply = self.send_worker(
            &job.account,
            &object(vec![
                ("cmd", str_value("run")),
                ("script", str_value(&script)),
                ("args", Value::Arr(args)),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        match reply {
            Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
                job.apply_session(reply.get("session"));
            }
            Ok(reply) => {
                let mut inner = job.inner.lock().unwrap();
                inner.last_error = Some(object(vec![
                    ("where", str_value("resume")),
                    ("type", str_value("resume")),
                    ("message", reply.get("error").clone()),
                    ("at", Value::Arr(vec![])),
                ]));
                inner.status = "failed".to_string();
            }
            Err(error) => {
                let mut inner = job.inner.lock().unwrap();
                inner.last_error = Some(object(vec![
                    ("where", str_value("resume")),
                    ("type", str_value("resume")),
                    ("message", str_value(&error)),
                    ("at", Value::Arr(vec![])),
                ]));
                inner.status = "failed".to_string();
            }
        }
    }

    /// Finished is transient: unload it back to idle.
    fn auto_idle_finished(&self, job: &Arc<Job>) {
        {
            let inner = job.inner.lock().unwrap();
            if inner.rescue_for.is_some() || inner.phase != "idle" || inner.status != "finished" {
                return;
            }
        }
        let reply = self.send_worker(
            &job.account,
            &object(vec![("cmd", str_value("unload"))]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        if let Ok(reply) = reply {
            if reply.get("ok").as_bool() == Some(true) {
                job.apply_session(reply.get("session"));
                let mut inner = job.inner.lock().unwrap();
                if inner.status == "finished" {
                    inner.status = "idle".to_string();
                }
            }
        }
    }

    fn handle_worker_death(&self, job: &Arc<Job>) {
        self.procs.lock().unwrap().remove(&job.account);
        let mut inner = job.inner.lock().unwrap();
        inner.supervise = false;
        inner.phase = "idle".to_string();
        inner.script = None;
        inner.status = "disconnected".to_string();
        inner.last_error = Some(object(vec![
            ("where", str_value("worker")),
            ("type", str_value("death")),
            (
                "message",
                str_value("session worker died: close + open to recover"),
            ),
            ("at", Value::Arr(vec![])),
        ]));
        println!("job {}: worker died", job.account);
        drop(inner);
        self.write_state(job);
    }
}
