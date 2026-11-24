#![forbid(unsafe_code)]
//! Supervisor daemon: one persistent session worker per account.
//! (Socket serving lives in main.rs, supervision in supervise.rs.)
//!
//! It owns session lifetime (open/close), the in-memory snapshots the
//! workers push, and the unix-socket dispatch the CLI talks to.
//! It never speaks the game protocol.

use std::collections::HashMap;
use std::io::Write;
pub(crate) use crate::daemon_conn::{is_pktlog_line, live_binary, tail_lines};
use std::process::{Child, Command};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::frame_codec::{Frame, FrameError, SessionFacts};
use crate::jobs::{Job, now_secs};
use crate::worker_call::CallError;
use crate::worker_command::WorkerCommand;
use lifescape_json::{self as json, Value, object, str_value};
use crate::paths;
use crate::registry;

pub use crate::client::WORKER_TIMEOUT;

pub(crate) use crate::daemon_watch::Watchdog;

const STOP_GRACE: f64 = 8.0;
pub const OPEN_TIMEOUT: f64 = 150.0;
// Process-side bounds in wall-clock seconds: spawning a worker and waiting
// for its socket RPCs happen outside any game connection, so no game-tick
// bound can apply here.

pub(crate) enum WorkerProc {
    Owned(Child),
    /// A worker that outlived the daemon: signalled by pid via /bin/kill.
    Attached(u32),
}

impl WorkerProc {
    fn alive(&mut self) -> bool {
        match self {
            WorkerProc::Owned(child) => child.try_wait().ok().flatten().is_none(),
            WorkerProc::Attached(pid) => std::path::Path::new(&format!("/proc/{pid}")).exists(),
        }
    }

    pub(crate) fn pid(&mut self) -> Option<u32> {
        // Pid 0 (a restored worker that reported none) is never signalled:
        // `kill -TERM 0` would hit the daemon's whole process group.
        match self {
            WorkerProc::Owned(child) => Some(child.id()).filter(|pid| *pid != 0),
            WorkerProc::Attached(pid) => Some(*pid).filter(|pid| *pid != 0),
        }
    }

    fn terminate_gracefully(&mut self) {
        if !self.alive() {
            return;
        }
        let pid = self.pid();
        // std has no SIGTERM: ask politely via /bin/kill, then SIGKILL
        // after a grace delay (STOP_GRACE).
        if let Some(pid) = pid {
            let _ = Command::new("kill").arg("-TERM").arg(pid.to_string()).output();
        }
        let deadline = now_secs() + STOP_GRACE;
        while now_secs() < deadline {
            if !self.alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        match self {
            WorkerProc::Owned(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            WorkerProc::Attached(pid) => {
                if *pid != 0 {
                    let _ = Command::new("kill").arg("-KILL").arg(pid.to_string()).output();
                }
            }
        }
    }
}

pub struct Orchestrator {
    pub jobs: Mutex<HashMap<String, Arc<Job>>>,
    pub(crate) procs: Mutex<HashMap<String, WorkerProc>>,
    /// One open/close/redeploy/respawn per account (`worker_owner`).
    pub(crate) lifecycle: crate::worker_owner::LifecycleLocks,
    pub running: AtomicBool,
    /// The configuration `main` read once (lot 5.10).
    pub config: crate::config::Config,
}


impl crate::fleet::Fleet for Orchestrator {
    fn jobs(&self) -> &Mutex<HashMap<String, Arc<Job>>> {
        &self.jobs
    }

    fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn proc_alive(&self, account: &str) -> bool {
        Orchestrator::proc_alive(self, account)
    }

    fn send_worker(&self, account: &str, request: &Value, timeout: Duration) -> Result<Value, CallError> {
        Orchestrator::send_worker(self, account, request, timeout)
    }

    fn run_job(&self, account: &str, script: &str, args: &[Value]) -> Value {
        Orchestrator::run_job(self, account, script, args)
    }

    fn open_job(self: Arc<Self>, account: &str, no_login: bool) -> Value {
        Orchestrator::open_job(&self, account, no_login)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Orchestrator {
    pub fn new() -> Self {
        Orchestrator::with_config(crate::config::Config::default())
    }

    pub fn with_config(config: crate::config::Config) -> Self {
        paths::ensure_dirs();
        Orchestrator {
            jobs: Mutex::new(HashMap::new()),
            procs: Mutex::new(HashMap::new()),
            lifecycle: crate::worker_owner::LifecycleLocks::default(),
            running: AtomicBool::new(true),
            config,
        }
    }

    pub(crate) fn proc_alive(&self, account: &str) -> bool {
        match self.procs.lock().unwrap().get_mut(account) {
            Some(proc) => proc.alive(),
            None => false,
        }
    }

    pub(crate) fn kill_process(&self, account: &str) {
        if let Some(mut proc) = self.procs.lock().unwrap().remove(account) {
            proc.terminate_gracefully();
        }
    }


    /// Worker binary for `session` spawn: `current_exe()` keeps pointing
    /// at the replaced file after `cargo build` ("/proc/self/exe" reads
    /// "... (deleted)"), so strip that marker and require the path to
    /// exist. Loud typed error, never a silent fallback: the operator
    /// restarts the daemon off the fresh build.
    fn worker_binary(account: &str) -> Result<std::path::PathBuf, String> {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        live_binary(&exe).ok_or_else(|| {
            format!(
                "worker binary gone at {} (daemon runs a deleted build; `orchestrator shutdown`, rebuild, then `open {account}` again)",
                exe.to_string_lossy()
            )
        })
    }

    /// Start the session worker and wait for its login (or typed error).
    /// Callers hold the account's lifecycle claim. Never beside a live
    /// worker: its socket, ready file and `procs` entry stay untouched.
    pub(crate) fn spawn(&self, job: &Arc<Job>, no_login: bool) -> Result<(), String> {
        if self.proc_alive(&job.account) {
            let pid = self.procs.lock().unwrap().get_mut(&job.account).and_then(WorkerProc::pid).unwrap_or(0);
            return Err(format!("worker pid {pid} for {} is still alive, refusing a second one", job.account));
        }
        if let Some(pid) = crate::worker_owner::live_workers(&job.account).first() {
            return Err(crate::worker_owner::foreign_worker_error(&job.account, *pid));
        }
        let (sock_path, ready_path) = paths::session_paths(&job.account);
        for stale in [&sock_path, &ready_path] {
            let _ = std::fs::remove_file(stale);
        }
        let (_merged, partial, log_path) = paths::user_paths(&job.account);
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| error.to_string())?;
        let exe = Self::worker_binary(&job.account)?;
        let mut command = Command::new(exe);
        command
            .arg("session")
            .arg(&job.account)
            .current_dir(paths::root())
            .env("LIFESCAPE_USER", &job.account)
            .env("LIFESCAPE_STATE", &partial)
            .env("LIFESCAPE_SERVER_HOST", &self.config.server_host)
            .env("LIFESCAPE_SERVER_PORT", self.config.server_port.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log);
        if no_login {
            command.env("LIFESCAPE_NO_LOGIN", "1");
        }
        let child = command.spawn().map_err(|error| error.to_string())?;
        let pid = child.id();
        self.procs
            .lock()
            .unwrap()
            .insert(job.account.clone(), WorkerProc::Owned(child));
        {
            let mut inner = job.inner.lock().unwrap();
            inner.status = crate::job_state::Status::Starting;
            inner.phase_at = now_secs();
            inner.started = now_secs();
            inner.session_pid = Some(pid);
        }
        crate::logln!("session {}: worker pid={pid}", job.account);
        let deadline = now_secs() + OPEN_TIMEOUT;
        let mut ready: Option<Value> = None;
        while now_secs() < deadline {
            if !self.proc_alive(&job.account) {
                break;
            }
            if let Some(value) = paths::read_json(&ready_path) {
                ready = Some(value);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&ready_path);
        let ok = ready.as_ref().and_then(|v| v.get("ok").as_bool()).unwrap_or(false);
        if !ok {
            let error = ready
                .as_ref()
                .and_then(|v| v.get("error").as_str().map(str::to_string))
                .unwrap_or_else(|| {
                    if self.proc_alive(&job.account) {
                        format!("open timed out after {}s (is {} logged in elsewhere?)", OPEN_TIMEOUT as u64, job.account)
                    } else {
                        "worker died during open".to_string()
                    }
                });
            self.kill_process(&job.account);
            let mut handle = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|error| error.to_string())?;
            let _ = writeln!(handle, "open failed for {}: {error}", job.account);
            return Err(error);
        }
        Ok(())
    }

    pub fn open_job(self: &Arc<Self>, account: &str, no_login: bool) -> Value {
        if account.is_empty() {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("account is required"))]);
        }
        // One lifecycle operation per account: the 20:17 race (operator
        // open + market-absent open) spawned two workers for ryan_park.
        // "already open" keeps market-absent waiting, never failing.
        let Some(_claim) = self.lifecycle.try_claim(account) else {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("already open {account}: another open/close is in flight"))),
            ]);
        };
        self.open_claimed(account, no_login)
    }

    /// `open_job` body, lifecycle claim held by the caller (open, redeploy).
    pub(crate) fn open_claimed(self: &Arc<Self>, account: &str, no_login: bool) -> Value {
        // No session admission (user 2026-11-15): every open goes
        // through at once; the disk line only warns (D34).
        {
            // D34 warning only (arbitration): log + res-stats flag, the
            // open goes through.
            let free = crate::disk_guard::free_disk_bytes(&crate::paths::run_dir());
            crate::res_stats::record_disk(free);
            if let Some(warning) = crate::disk_guard::disk_warning(free) {
                crate::logln!("open {account}: {warning}");
            }
        }
        // The worker processes are the truth, tracked or not: a live one
        // is either ours (already open), being adopted by restore_jobs
        // (answers its socket) or foreign (typed refusal naming its pid).
        // Its socket is never removed here.
        let tracked_alive = self.jobs.lock().unwrap().contains_key(account) && self.proc_alive(account);
        let live = crate::worker_owner::live_workers(account);
        let answers = !tracked_alive
            && !live.is_empty()
            && self.send_worker(account, &WorkerCommand::Ping.encode(), Duration::from_secs(2)).is_ok();
        match crate::worker_owner::open_verdict(tracked_alive, &live, answers) {
            crate::worker_owner::OpenVerdict::Spawn => {}
            crate::worker_owner::OpenVerdict::AlreadyOpen => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("already open {account}"))),
                ]);
            }
            crate::worker_owner::OpenVerdict::Foreign(pid) => {
                crate::logln!("open {account}: refused, untracked live worker pid {pid}");
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&crate::worker_owner::foreign_worker_error(account, pid))),
                ]);
            }
        }
        self.jobs.lock().unwrap().remove(account);
        // An `open` replaces whatever worker ran here (operator redeploy,
        // reopen after a kill): the node it was running ends without a
        // report, which is a stop, never a crash (live 16:48: a fleet
        // redeploy cost each active node one failure, and the stopped
        // report pushed just before rode the discarded job).
        crate::brain_grace::note_operator_stop(account);
        let job = Arc::new(Job::new(account));
        if let Err(error) = self.spawn(&job, no_login) {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]);
        }
        self.jobs.lock().unwrap().insert(account.to_string(), job.clone());
        if let Ok(reply) = self.send_worker(
            account,
            &WorkerCommand::Status.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        ) {
            job.apply_session(reply.get("session"));
        }
        self.write_state(&job);
        self.start_supervisor(job.clone());
        object(vec![("ok", Value::Bool(true)), ("job", job.describe())])
    }

    pub(crate) fn run_job(&self, account: &str, script: &str, args: &[Value]) -> Value {
        let job = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session, open {account} first"))),
                ]);
            }
        };
        if self.drop_dead_job(account) {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("session for {account} died, open {account} again"))),
            ]);
        }
        if !registry::script_known(script) {
            return object(vec![
                ("ok", Value::Bool(false)),
                (
                    "error",
                    str_value(&format!(
                        "unknown script '{script}' (known: {})",
                        registry::script_names_sorted().join(", ")
                    )),
                ),
            ]);
        }
        let reply = self.send_worker(
            account,
            &WorkerCommand::Run { script: script.to_string(), args: args.to_vec() }.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(error.text()))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            self.write_state(&job);
        }
        reply
    }

    /// Manual hold: stop the running script and let the worker idle drain
    /// run no rescue until resumed. The session stays open for manual
    /// `call` piloting.
    pub(crate) fn hold_job(&self, account: &str) -> Value {
        let job = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session, open {account} first"))),
                ]);
            }
        };
        job.inner.lock().unwrap().held = true;
        let reply = self.send_worker(
            account,
            &WorkerCommand::Hold.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(error.text()))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            job.inner.lock().unwrap().held = true;
            self.write_state(&job);
        }
        reply
    }

    /// Release a manual hold (and resume a worker pause when there is one).
    pub(crate) fn resume_job(&self, account: &str) -> Value {
        let job = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session, open {account} first"))),
                ]);
            }
        };
        let was_held = std::mem::replace(&mut job.inner.lock().unwrap().held, false);
        let reply = self.simple_worker_cmd(account, WorkerCommand::Resume);
        if reply.get("ok").as_bool() == Some(true) {
            return reply;
        }
        if was_held {
            self.write_state(&job);
            return object(vec![("ok", Value::Bool(true)), ("job", job.describe())]);
        }
        reply
    }

    pub(crate) fn simple_worker_cmd(&self, account: &str, command: WorkerCommand) -> Value {
        let job = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session for {account}"))),
                ]);
            }
        };
        let reply = self.send_worker(
            account,
            &command.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(error.text()))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            self.write_state(&job);
        }
        reply
    }

    pub(crate) fn call_job(&self, account: &str, method: &str, args: &[Value]) -> Value {
        if self.jobs.lock().unwrap().get(account).is_none() {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("no session, open {account} first"))),
            ]);
        }
        if self.drop_dead_job(account) {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("session for {account} died, open {account} again"))),
            ]);
        }
        self.send_worker(
            account,
            &WorkerCommand::Call { method: method.to_string(), args: args.to_vec(), admin: crate::economy_guard::is_staff(account) }.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        )
        .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(error.text()))]))
    }

    pub(crate) fn methods_job(&self, account: &str) -> Value {
        if self.jobs.lock().unwrap().get(account).is_none() {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("no session, open {account} first"))),
            ]);
        }
        self.send_worker(
            account,
            &WorkerCommand::Methods { admin: crate::economy_guard::is_staff(account) }.encode(),
            Duration::from_secs(WORKER_TIMEOUT),
        )
        .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(error.text()))]))
    }

    /// Drop a job whose worker is gone and say so plainly, instead of
    /// letting every later command fail with "session unreachable".
    fn drop_dead_job(&self, account: &str) -> bool {
        if self.proc_alive(account) {
            return false;
        }
        self.jobs.lock().unwrap().remove(account);
        true
    }



    pub(crate) fn write_state(&self, job: &Arc<Job>) {
        let (merged_path, _partial, _log) = paths::user_paths(&job.account);
        let _ = paths::write_json(&merged_path, &job.describe());
        crate::res_stats::record_file();
    }

    /// Worker push frames (`session_push`): snapshots refresh the memory
    /// cache the front reads, events update telemetry and persist the
    /// merged state at once. Fire-and-forget from the worker side: a lost
    /// frame is a missed heartbeat, the next pump pushes again.
    pub fn ingest_push(&self, frame: &Value) -> Value {
        self.ingest_push_sized(frame, json::stringify(frame).len() as u64)
    }

    /// `ingest_push` with the frame's byte count already known (the push
    /// hub passes the length of the line it received: no re-serialisation
    /// of every frame just to count it, lot 1.3).
    pub fn ingest_push_sized(&self, frame: &Value, bytes: u64) -> Value {
        self.ingest_push_owned(frame.clone(), bytes)
    }

    /// `ingest_push_sized` for a frame the caller gives up (the push hub):
    /// a full snapshot's tree is MOVED into the job, never copied out of
    /// the frame (lot 1.5, 35 to 85 KB per bot every few seconds). The
    /// line is decoded once, here (`frame_codec`, lot 5.2).
    pub fn ingest_push_owned(&self, frame: Value, bytes: u64) -> Value {
        self.ingest_frame(Frame::decode(frame), bytes)
    }

    /// The push hub's entry (lot 4.3): ingest the frame, then, for a task
    /// report, wake the brain for that account at once. The supervisor's
    /// drain stays as the fallback; `Job::take_report` gives the report to
    /// whoever asks first.
    pub fn ingest_push_and_wake(self: &Arc<Self>, frame: Value, bytes: u64) -> Value {
        let decoded = Frame::decode(frame);
        let report_for = match &decoded {
            Ok(Frame::TaskReport(report)) => Some(report.account.clone()),
            _ => None,
        };
        let reply = self.ingest_frame(decoded, bytes);
        if let Some(account) = report_for {
            if reply.get("ok").as_bool() == Some(true) {
                let job = self.jobs.lock().unwrap().get(&account).cloned();
                if let Some(job) = job {
                    let fleet: Arc<dyn crate::fleet::Fleet> = self.clone();
                    crate::brain::on_report_push(&fleet, &job);
                }
            }
        }
        reply
    }

    /// Lot 4.2: the phase and status a heartbeat frame carries.
    fn note_frame_state(&self, job: &Arc<Job>, facts: Option<SessionFacts>) {
        // Lot 4.4: a phase turning idle asks the brain for a decision now
        // (the report may have been folded a few ms before the frame).
        if facts.and_then(|facts| job.note_frame(&facts, now_secs())).is_some_and(|effect| effect.became_idle) {
            crate::wake_hub::post(&job.account, crate::wake_hub::Event::Idle);
        }
    }

    /// The ingest of a decoded frame.
    fn ingest_frame(&self, decoded: Result<Frame, FrameError>, bytes: u64) -> Value {
        crate::res_stats::record_push(bytes);
        let account = match &decoded {
            Ok(frame) => frame.account().to_string(),
            Err(error) => error.account().to_string(),
        };
        if account.is_empty() {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value("account is required"))]);
        }
        let job = match self.jobs.lock().unwrap().get(&account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session for {account}"))),
                ]);
            }
        };
        let frame = match decoded {
            Ok(frame) => frame,
            Err(FrameError::UnknownType { kind, .. }) => {
                return object(vec![("ok", Value::Bool(false)), ("error", str_value(&format!("unknown push type {kind:?}")))]);
            }
            Err(FrameError::NotAnObject) => unreachable!("a non-object frame names no account"),
        };
        let facts = frame.session_facts();
        match frame {
            Frame::Snapshot(full) => {
                // The tree is MOVED into the job (lot 1.5).
                if let Ok(mut inner) = job.inner.try_lock() {
                    inner.live = Some(Arc::new(full.snapshot));
                }
                self.note_frame_state(&job, facts);
                object(vec![("ok", Value::Bool(true))])
            }
            Frame::Light(light) => {
                // E4 two-flow: merge the light keys into the last full
                // state (or hold the light alone before the first full).
                crate::res_stats::record_light();
                if let Ok(mut inner) = job.inner.try_lock() {
                    match inner.live.as_mut() {
                        // Copy-on-write: the tree is copied only while a
                        // reader still holds the previous pointer.
                        Some(cached) => light.merge_into(Arc::make_mut(cached)),
                        None => inner.live = Some(Arc::new(light.encode())),
                    }
                }
                self.note_frame_state(&job, facts);
                object(vec![("ok", Value::Bool(true))])
            }
            Frame::Event(event) => {
                let (kind, name, resolved, tick) = (event.kind.as_str(), event.event.as_str(), event.resolved, event.tick);
                let message = event.message.as_str();
                if message.is_empty() {
                    crate::logln!("job {account}: event {name} kind={kind} resolved={resolved} t={tick} (push)");
                } else {
                    crate::logln!("job {account}: event {name} kind={kind} resolved={resolved} t={tick} (push): {message}");
                }
                crate::errors_ingest::observe_event(&account, &event);
                {
                    let Ok(mut inner) = job.inner.try_lock() else {
                        return object(vec![("ok", Value::Bool(false)), ("error", str_value("contended"))]);
                    };
                    if kind == "script_error" {
                        inner.last_error = Some(object(vec![
                            ("where", str_value(&event.script)),
                            ("type", str_value("script_error")),
                            ("message", str_value(&event.message)),
                            ("at", event.at.clone()),
                            ("at_unix", Value::Float(now_secs())),
                            ("tick", Value::Int(tick)),
                        ]));
                    }
                    let mut entry = std::collections::BTreeMap::new();
                    entry.insert("event".to_string(), str_value(name));
                    entry.insert("resolved".to_string(), Value::Bool(resolved));
                    entry.insert("args".to_string(), Value::Arr(event.args.clone()));
                    entry.insert("at".to_string(), Value::Float(now_secs()));
                    inner.last_event = Some(Value::Obj(entry));
                }
                self.write_state(&job);
                object(vec![("ok", Value::Bool(true))])
            }
            Frame::TaskReport(carried) => {
                let report = carried.report.clone();
                crate::logln!(
                    "job {account}: task_report stop={} activity={} ticks={} (push)",
                    report.get("stop").as_str().unwrap_or("?"),
                    report.get("activity").as_str().unwrap_or("?"),
                    report.get("ticks").as_i64().unwrap_or(-1),
                );
                crate::cpu_profile::note_report_received(&account);
                // Flag the single supervisor (brain trigger) immediately. The
                // report reaches the store through the brain (`rates_ingest`);
                // the `lifescape-staging.jsonl` log nobody read is gone (lot 1.4).
                crate::errors_ingest::observe_report(&account, &report, &job);
                let Ok(mut inner) = job.inner.try_lock() else {
                    return object(vec![("ok", Value::Bool(false)), ("error", str_value("contended"))]);
                };
                inner.last_report = Some(report);
                inner.report_pending = true;
                self.write_state(&job);
                object(vec![("ok", Value::Bool(true))])
            }
        }
    }

    /// Adopt live session workers as idle, without their scripts.
    pub fn restore_jobs(self: &Arc<Self>) {
        for account in known_accounts() {
            if self.jobs.lock().unwrap().contains_key(&account) {
                continue;
            }
            let (sock_path, _ready) = paths::session_paths(&account);
            if !sock_path.exists() {
                continue;
            }
            // A worker busy in a script answers late: one longer retry
            // before giving up on it (never an orphan, see send_worker).
            let status = WorkerCommand::Status.encode();
            let reply = self
                .send_worker(&account, &status, Duration::from_secs(3))
                .or_else(|_| self.send_worker(&account, &status, Duration::from_secs(10)));
            let mut session = match reply {
                Ok(reply) => reply.get("session").clone(),
                Err(error) => {
                    // Only a socket nobody listens on is stale. A slow but
                    // live worker (busy at restart) keeps its socket: removing
                    // it orphaned the worker, still logged in, unreachable.
                    let dead = error.is_socket_stale();
                    if dead {
                        let _ = std::fs::remove_file(&sock_path);
                    } else {
                        crate::logln!("restore {account}: worker not answering ({error}), socket kept for the next pass");
                    }
                    continue;
                }
            };
            if session.as_obj().is_none() {
                continue;
            }
            if session.get("phase").as_str() == Some("running") {
                if self
                    .send_worker(
                        &account,
                        &WorkerCommand::Unload.encode(),
                        Duration::from_secs(WORKER_TIMEOUT),
                    )
                    .is_ok()
                {
                    if let Ok(reply) = self.send_worker(
                        &account,
                        &WorkerCommand::Status.encode(),
                        Duration::from_secs(3),
                    ) {
                        session = reply.get("session").clone();
                    }
                }
            }
            if self.jobs.lock().unwrap().contains_key(&account) {
                continue;
            }
            let job = Arc::new(Job::new(&account));
            self.procs.lock().unwrap().insert(
                account.clone(),
                WorkerProc::Attached(session.get("session_pid").as_i64().unwrap_or(0) as u32),
            );
            job.apply_session(&session);
            {
                let mut inner = job.inner.lock().unwrap();
                inner.phase = crate::job_state::Phase::Idle;
                inner.script = None;
                inner.script_args = Vec::new();
                if inner.status == crate::job_state::Status::Running {
                    inner.status = crate::job_state::Status::Idle;
                }
            }
            self.jobs.lock().unwrap().insert(account.clone(), job.clone());
            self.start_supervisor(job);
            crate::logln!("restored {account} (idle)");
        }
    }
}

pub(crate) fn known_accounts() -> Vec<String> {
    let mut accounts = Vec::new();
    if let Ok(names) = std::fs::read_dir(paths::users_dir()) {
        for entry in names.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") && !name.ends_with(".bot.json") {
                accounts.push(name[..name.len() - 5].to_string());
            }
        }
    }
    accounts
}

