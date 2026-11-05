#![forbid(unsafe_code)]
//! Supervisor daemon: one persistent session worker per account.
//! Mirrors orchestration/daemon.py (socket serving lives in main.rs,
//! supervision in supervise.rs).
//!
//! It owns session lifetime (open/close), the log tail that carries
//! ``@@EVENT`` markers, and the unix-socket dispatch the CLI talks to.
//! It never speaks the game protocol.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::jobs::{Job, now_secs, resolve_state};
use crate::json::{self, Value, object, str_value};
use crate::paths;
use crate::registry;

pub const WORKER_TIMEOUT: u64 = 40;

const STOP_GRACE: f64 = 8.0;
const OPEN_TIMEOUT: f64 = 150.0;
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

    fn pid(&mut self) -> Option<u32> {
        match self {
            WorkerProc::Owned(child) => child.id().into(),
            WorkerProc::Attached(pid) => Some(*pid),
        }
    }

    fn terminate_gracefully(&mut self) {
        let pid = self.pid();
        // std has no SIGTERM: ask politely via /bin/kill, then SIGKILL
        // after the same grace the Python daemon uses.
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
            }
            WorkerProc::Attached(pid) => {
                let _ = Command::new("kill").arg("-KILL").arg(pid.to_string()).output();
            }
        }
    }
}

pub struct Orchestrator {
    pub(crate) jobs: Mutex<HashMap<String, Arc<Job>>>,
    pub(crate) procs: Mutex<HashMap<String, WorkerProc>>,
    pub running: AtomicBool,
}

impl Orchestrator {
    pub fn new() -> Self {
        paths::ensure_dirs();
        Orchestrator {
            jobs: Mutex::new(HashMap::new()),
            procs: Mutex::new(HashMap::new()),
            running: AtomicBool::new(true),
        }
    }

    pub(crate) fn proc_alive(&self, account: &str) -> bool {
        match self.procs.lock().unwrap().get_mut(account) {
            Some(proc) => proc.alive(),
            None => false,
        }
    }

    fn kill_process(&self, account: &str) {
        if let Some(mut proc) = self.procs.lock().unwrap().remove(account) {
            proc.terminate_gracefully();
        }
    }

    /// Start the session worker and wait for its login (or typed error).
    fn spawn(&self, job: &Arc<Job>, no_login: bool) -> Result<(), String> {
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
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut command = Command::new(exe);
        command
            .arg("session")
            .arg(&job.account)
            .current_dir(paths::root())
            .env("LIFESCAPE_USER", &job.account)
            .env("LIFESCAPE_STATE", &partial)
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
            inner.status = "starting".to_string();
            inner.started = now_secs();
            inner.session_pid = Some(pid);
        }
        println!("session {}: worker pid={pid}", job.account);
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
        {
            let jobs = self.jobs.lock().unwrap();
            if jobs.contains_key(account) && self.proc_alive(account) {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("already open {account}"))),
                ]);
            }
        }
        self.jobs.lock().unwrap().remove(account);
        let (sock_path, _ready) = paths::session_paths(account);
        if sock_path.exists() {
            match self.send_worker(account, &object(vec![("cmd", str_value("ping"))]), Duration::from_secs(2)) {
                Ok(_) => {
                    return object(vec![
                        ("ok", Value::Bool(false)),
                        ("error", str_value(&format!("already open {account}"))),
                    ]);
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&sock_path);
                }
            }
        }
        let job = Arc::new(Job::new(account));
        if let Err(error) = self.spawn(&job, no_login) {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]);
        }
        self.jobs.lock().unwrap().insert(account.to_string(), job.clone());
        if let Ok(reply) = self.send_worker(
            account,
            &object(vec![("cmd", str_value("status"))]),
            Duration::from_secs(WORKER_TIMEOUT),
        ) {
            job.apply_session(reply.get("session"));
        }
        self.write_state(&job);
        self.start_supervisor(job.clone());
        object(vec![("ok", Value::Bool(true)), ("job", job.describe())])
    }

    fn run_job(&self, account: &str, script: &str, args: &[Value]) -> Value {
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
        if job.inner.lock().unwrap().rescue_for.is_some() {
            let rescue = job.inner.lock().unwrap().rescue_for.clone().unwrap_or_default();
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("busy: rescue {rescue} in progress"))),
            ]);
        }
        if let Err(error) = registry::script_path(script) {
            return object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]);
        }
        let reply = self.send_worker(
            account,
            &object(vec![
                ("cmd", str_value("run")),
                ("script", str_value(script)),
                ("args", Value::Arr(args.to_vec())),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            self.write_state(&job);
        }
        reply
    }

    /// Manual hold: stop the running script and run no rescue until
    /// resumed. The session stays open for manual `call` piloting.
    fn hold_job(&self, account: &str) -> Value {
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
            &object(vec![("cmd", str_value("unload"))]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            job.inner.lock().unwrap().held = true;
            self.write_state(&job);
        }
        reply
    }

    /// Release a manual hold (and resume a worker pause when there is one).
    fn resume_job(&self, account: &str) -> Value {
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
        let reply = self.simple_worker_cmd(account, "resume");
        if reply.get("ok").as_bool() == Some(true) {
            return reply;
        }
        if was_held {
            self.write_state(&job);
            return object(vec![("ok", Value::Bool(true)), ("job", job.describe())]);
        }
        reply
    }

    fn simple_worker_cmd(&self, account: &str, cmd: &str) -> Value {        let job = match self.jobs.lock().unwrap().get(account).cloned() {
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
            &object(vec![("cmd", str_value(cmd))]),
            Duration::from_secs(WORKER_TIMEOUT),
        );
        let reply = reply.unwrap_or_else(|error| {
            object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))])
        });
        if reply.get("ok").as_bool() == Some(true) {
            job.apply_session(reply.get("session"));
            self.write_state(&job);
        }
        reply
    }

    fn call_job(&self, account: &str, method: &str, args: &[Value]) -> Value {
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
            &object(vec![
                ("cmd", str_value("call")),
                ("method", str_value(method)),
                ("args", Value::Arr(args.to_vec())),
                ("admin", Value::Bool(account == "admin")),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        )
        .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]))
    }

    fn methods_job(&self, account: &str) -> Value {
        if self.jobs.lock().unwrap().get(account).is_none() {
            return object(vec![
                ("ok", Value::Bool(false)),
                ("error", str_value(&format!("no session, open {account} first"))),
            ]);
        }
        self.send_worker(
            account,
            &object(vec![
                ("cmd", str_value("methods")),
                ("admin", Value::Bool(account == "admin")),
            ]),
            Duration::from_secs(WORKER_TIMEOUT),
        )
        .unwrap_or_else(|error| object(vec![("ok", Value::Bool(false)), ("error", str_value(&error))]))
    }

    fn close_job(&self, account: &str) -> Value {
        let job = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job,
            None => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value(&format!("no session for {account}"))),
                ]);
            }
        };
        job.inner.lock().unwrap().supervise = false;
        let mut session = Value::Null;
        match self.send_worker(
            account,
            &object(vec![("cmd", str_value("close"))]),
            Duration::from_secs(WORKER_TIMEOUT),
        ) {
            Ok(reply) if reply.get("ok").as_bool() == Some(true) => {
                session = reply.get("session").clone();
            }
            Ok(_) => {}
            Err(error) => println!("close {account}: worker unreachable: {error}"),
        }
        self.kill_process(account);
        let (sock, _ready) = paths::session_paths(account);
        let _ = std::fs::remove_file(sock);
        {
            let mut inner = job.inner.lock().unwrap();
            inner.status = "stopped".to_string();
            inner.phase = "idle".to_string();
            inner.script = None;
            inner.paused = false;
            inner.supervise = false;
            if !session.get("last_error").is_null() {
                inner.last_error = Some(session.get("last_error").clone());
            }
        }
        self.write_state(&job);
        self.jobs.lock().unwrap().remove(account);
        object(vec![("ok", Value::Bool(true)), ("job", job.describe())])
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

    /// One JSON round-trip to the session worker.
    pub(crate) fn send_worker(&self, account: &str, request: &Value, timeout: Duration) -> Result<Value, String> {
        let (sock_path, _ready) = paths::session_paths(account);
        let stream = UnixStream::connect(&sock_path)
            .map_err(|_| format!("session unreachable for {account}"))?;
        stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let mut buf = json::stringify(request);
        buf.push('\n');
        (&stream).write_all(buf.as_bytes()).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            return Ok(object(vec![]));
        }
        json::parse(&line).map_err(|e| e.to_string())
    }

    pub(crate) fn write_state(&self, job: &Arc<Job>) {
        let (merged_path, _partial, _log) = paths::user_paths(&job.account);
        let _ = paths::write_json(&merged_path, &job.describe());
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
            let mut session = match self.send_worker(
                &account,
                &object(vec![("cmd", str_value("status"))]),
                Duration::from_secs(3),
            ) {
                Ok(reply) => reply.get("session").clone(),
                Err(_) => {
                    let _ = std::fs::remove_file(&sock_path);
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
                        &object(vec![("cmd", str_value("unload"))]),
                        Duration::from_secs(WORKER_TIMEOUT),
                    )
                    .is_ok()
                {
                    if let Ok(reply) = self.send_worker(
                        &account,
                        &object(vec![("cmd", str_value("status"))]),
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
                inner.phase = "idle".to_string();
                inner.script = None;
                inner.script_args = Vec::new();
                if inner.status == "running" {
                    inner.status = "idle".to_string();
                }
                let (_merged, _partial, log_path) = paths::user_paths(&account);
                inner.log_offset = std::fs::metadata(&log_path).map(|meta| meta.len()).unwrap_or(0);
            }
            self.jobs.lock().unwrap().insert(account.clone(), job.clone());
            self.start_supervisor(job);
            println!("restored {account} (idle)");
        }
    }

    pub fn dispatch(&self, request: &Value) -> Value {
        let command = request.get("cmd").as_str().unwrap_or("");
        let account = request.get("account").as_str().unwrap_or("");
        match command {
            "open" => {
                return object(vec![
                    ("ok", Value::Bool(false)),
                    ("error", str_value("open needs daemon context")),
                ]);
            }
            "run" => {
                let script = request.get("script").as_str().unwrap_or("");
                let args = request.get("args").as_arr().cloned().unwrap_or_default();
                return self.run_job(account, script, &args);
            }
            "unload" => return self.simple_worker_cmd(account, "unload"),
            "pause" => return self.simple_worker_cmd(account, "pause"),
            "resume" => return self.resume_job(account),
            "hold" => return self.hold_job(account),
            "close" => return self.close_job(account),
            "call" => {
                let method = request.get("method").as_str().unwrap_or("");
                let args = request.get("args").as_arr().cloned().unwrap_or_default();
                // CLI sends raw strings; the worker parses JSON-or-string.
                let args: Vec<Value> = args
                    .iter()
                    .map(|arg| arg.as_str().map(str_value).unwrap_or_else(|| arg.clone()))
                    .collect();
                return self.call_job(account, method, &args);
            }
            "methods" => return self.methods_job(account),
            "status" => {
                let live: HashMap<String, Arc<Job>> = self.jobs.lock().unwrap().clone();
                let mut names: Vec<String> = if account.is_empty() {
                    live.keys().cloned().collect()
                } else if live.contains_key(account) {
                    vec![account.to_string()]
                } else {
                    Vec::new()
                };
                names.sort();
                let mut described: Vec<Value> = names
                    .iter()
                    .filter_map(|name| live.get(name))
                    .map(|job| {
                        let mut entry = job.describe();
                        if let Value::Obj(map) = &mut entry {
                            map.insert(
                                "account_name".to_string(),
                                str_value(&crate::jobs::humanize(job.account.as_str())),
                            );
                        }
                        entry
                    })
                    .collect();
                described.sort_by(|a, b| {
                    let idle = |v: &Value| v.get("status").as_str() == Some("idle");
                    (idle(a), a.get("account").as_str().unwrap_or(""))
                        .cmp(&(idle(b), b.get("account").as_str().unwrap_or("")))
                });
                return object(vec![("ok", Value::Bool(true)), ("jobs", Value::Arr(described))]);
            }
            "items" => return object(vec![("ok", Value::Bool(true)), ("data", holdings_snapshot())]),
            "scripts" => {
                return object(vec![
                    ("ok", Value::Bool(true)),
                    (
                        "scripts",
                        Value::Arr(registry::script_names_sorted().iter().map(|n| str_value(n)).collect()),
                    ),
                ]);
            }
            "accounts" => {
                let live: Vec<String> = {
                    let jobs = self.jobs.lock().unwrap();
                    let mut names: Vec<String> = jobs.keys().cloned().collect();
                    names.sort();
                    names
                };
                let mut known = known_accounts();
                for name in &live {
                    if !known.contains(name) {
                        known.push(name.clone());
                    }
                }
                known.sort();
                return object(vec![
                    ("ok", Value::Bool(true)),
                    ("accounts", Value::Arr(known.iter().map(|n| str_value(n)).collect())),
                    ("live", Value::Arr(live.iter().map(|n| str_value(n)).collect())),
                ]);
            }
            "show" => {
                let (merged_path, _partial, _log) = paths::user_paths(account);
                let state = paths::read_json(&merged_path).unwrap_or(Value::Null);
                let state = if state.as_obj().is_some() { state } else { object(vec![]) };
                return object(vec![("ok", Value::Bool(true)), ("state", resolve_state(state))]);
            }
            "live" => {
                let (merged_path, partial_path, _log) = paths::user_paths(account);
                let merged = paths::read_json(&merged_path).unwrap_or(Value::Null);
                let state = match (merged, paths::read_json(&partial_path)) {
                    (Value::Obj(mut base), Some(Value::Obj(extra))) => {
                        base.extend(extra);
                        Value::Obj(base)
                    }
                    (base, _) => base,
                };
                let state = if state.as_obj().is_some() { state } else { object(vec![]) };
                return object(vec![("ok", Value::Bool(true)), ("state", resolve_state(state))]);
            }
            "logs" => {
                let (_merged, _partial, log_path) = paths::user_paths(account);
                let lines = request.get("lines").as_i64().unwrap_or(40).max(0) as usize;
                let tail = tail_lines(&log_path, lines);
                return object(vec![
                    ("ok", Value::Bool(true)),
                    ("lines", Value::Arr(tail.iter().map(|line| str_value(line)).collect())),
                ]);
            }
            "shutdown" => {
                let accounts: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
                for name in accounts {
                    let _ = self.close_job(&name);
                }
                self.running.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = UnixStream::connect(paths::socket_path());
                return object(vec![("ok", Value::Bool(true))]);
            }
            _ => {}
        }
        object(vec![
            ("ok", Value::Bool(false)),
            ("error", str_value(&format!("unknown command '{command}'"))),
        ])
    }
}

/// Same keys as holdings.snapshot(); history sampling (sqlite) is not
/// ported, so samples/total stay empty and items carry no rows.
fn holdings_snapshot() -> Value {
    object(vec![
        ("intervalMinutes", Value::Int(30)),
        ("samples", Value::Arr(vec![])),
        ("total", Value::Arr(vec![])),
        ("liveTotal", Value::Int(0)),
        ("items", Value::Arr(vec![])),
    ])
}

fn known_accounts() -> Vec<String> {
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

/// Last lines of a (possibly huge) log without reading it whole.
fn tail_lines(path: &std::path::Path, lines: usize) -> Vec<String> {
    const WINDOW: u64 = 131072;
    use std::io::{Read, Seek};
    let mut handle = match std::fs::File::open(path) {
        Ok(handle) => handle,
        Err(_) => return Vec::new(),
    };
    let size = handle.seek(std::io::SeekFrom::End(0)).unwrap_or(0);
    let offset = size.saturating_sub(std::cmp::max(WINDOW, lines as u64 * 2048));
    if handle.seek(std::io::SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut data = Vec::new();
    if handle.read_to_end(&mut data).is_err() {
        return Vec::new();
    }
    let mut found: Vec<String> = String::from_utf8_lossy(&data).split('\n').map(str::to_string).collect();
    if offset > 0 && !found.is_empty() {
        found.remove(0);
    }
    if lines == 0 {
        return Vec::new();
    }
    // A trailing newline leaves a final empty element: drop it like
    // Python's splitlines() does.
    if found.last().is_some_and(|line| line.is_empty()) {
        found.pop();
    }
    if found.len() > lines {
        found[found.len() - lines..].to_vec()
    } else {
        found
    }
}
