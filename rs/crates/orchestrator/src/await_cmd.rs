#![forbid(unsafe_code)]
//! `run --wait` / `await`: wait for a script to end, print one VERDICT.
//!
//! Live-test usage: `orchestrator run <compte> <script> [args] --wait
//! [--timeout-s N] [--json]` starts the script then blocks until it ends;
//! `orchestrator await <compte> [--timeout-s N] [--json]` only blocks.
//! The daemon is polled every 300 ms with a 5 s socket timeout per poll:
//! the 40 s worker timeout never bounds the total wait.
//!
//! End-of-run detection (W5): the daemon `status` may stay `running` long
//! after the script ended, `last_code` may arrive only after `unload`,
//! and a failure may carry `last_error` with a null code. So a fresh
//! `last_code`/`last_error` (vs the pre-run baseline) ends the wait even
//! when the status is stale; status and phase only arbitrate the rest.
//! Exit codes: ok 0, budget 2, fail 1, wall-timeout 124, no-session 3.

use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

/// Poll cadence: inside the required 200-500 ms window.
pub const POLL_MS: u64 = 300;
/// Per-poll socket timeout: short, never the 40 s worker timeout.
const POLL_TIMEOUT: Duration = Duration::from_secs(5);
/// Default wall budget when `--timeout-s` is omitted.
pub const DEFAULT_TIMEOUT_S: f64 = 120.0;
/// Log lines carried by the VERDICT.
pub const LOG_TAIL: usize = 15;
/// Consecutive transport errors before giving up (daemon died mid-run).
const MAX_TRANSPORT_ERRORS: usize = 5;

#[derive(Debug)]
pub enum AwaitError {
    Transport(String),
    NoSession(String),
    BadReply(String),
}

impl std::fmt::Display for AwaitError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AwaitError::Transport(detail) => write!(out, "session unreachable: {detail}"),
            AwaitError::NoSession(detail) => write!(out, "no session: {detail}"),
            AwaitError::BadReply(detail) => write!(out, "bad daemon reply: {detail}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Budget,
    Fail,
    WallTimeout,
    NoSession,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Budget => "budget",
            Verdict::Fail => "fail",
            Verdict::WallTimeout => "wall-timeout",
            Verdict::NoSession => "no-session",
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Verdict::Ok => 0,
            Verdict::Budget => 2,
            Verdict::Fail => 1,
            Verdict::WallTimeout => 124,
            Verdict::NoSession => 3,
        }
    }
}

/// Flags accepted after the script args (`run`) or the account (`await`).
pub struct WaitFlags {
    pub wait: bool,
    pub json: bool,
    pub timeout_s: f64,
}

/// Split `--wait` / `--json` / `--timeout-s N|--timeout-s=N` out of the
/// trailing args; everything else stays a script arg. Unknown `--*`
/// fails loud: a typo must not silently become a script argument.
pub fn split_flags(args: &[String]) -> Result<(Vec<String>, WaitFlags), String> {
    let mut kept = Vec::new();
    let mut wait = false;
    let mut json = false;
    let mut timeout_s = DEFAULT_TIMEOUT_S;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--wait" {
            wait = true;
        } else if arg == "--json" {
            json = true;
        } else if arg == "--timeout-s" {
            index += 1;
            let raw = args.get(index).ok_or("missing value for --timeout-s".to_string())?;
            timeout_s = parse_timeout(raw)?;
        } else if let Some(raw) = arg.strip_prefix("--timeout-s=") {
            timeout_s = parse_timeout(raw)?;
        } else if arg.starts_with("--") {
            return Err(format!("unknown flag {arg:?}"));
        } else {
            kept.push(args[index].clone());
        }
        index += 1;
    }
    Ok((kept, WaitFlags { wait, json, timeout_s }))
}

fn parse_timeout(raw: &str) -> Result<f64, String> {
    let parsed: f64 = raw.parse().map_err(|_| format!("bad --timeout-s {raw:?}"))?;
    if parsed.is_finite() && parsed > 0.0 {
        Ok(parsed)
    } else {
        Err(format!("bad --timeout-s {raw:?}"))
    }
}

/// Whatever the waiter polls: the daemon socket live, a script in tests.
pub trait StateSource {
    fn show(&mut self) -> Result<Value, AwaitError>;
    fn logs(&mut self, lines: usize) -> Result<Vec<String>, AwaitError>;
}

pub struct DaemonSource {
    pub account: String,
}

impl StateSource for DaemonSource {
    fn show(&mut self) -> Result<Value, AwaitError> {
        let reply = crate::cli::send_command(
            &object(vec![("cmd", str_value("show")), ("account", str_value(&self.account))]),
            POLL_TIMEOUT,
        )
        .map_err(AwaitError::Transport)?;
        if reply.get("ok").as_bool() != Some(true) {
            return Err(AwaitError::BadReply(json::stringify(reply.get("error"))));
        }
        let state = reply.get("state").clone();
        // Never-opened accounts fall back to an empty state; closed
        // sessions keep their stopped file (a fail verdict, not no-session).
        if state.as_obj().is_none() || state.get("account").as_str().unwrap_or("").is_empty() {
            return Err(AwaitError::NoSession(format!("open {} first", self.account)));
        }
        Ok(state)
    }

    fn logs(&mut self, lines: usize) -> Result<Vec<String>, AwaitError> {
        let reply = crate::cli::send_command(
            &object(vec![
                ("cmd", str_value("logs")),
                ("account", str_value(&self.account)),
                ("lines", Value::Int(lines as i64)),
            ]),
            POLL_TIMEOUT,
        )
        .map_err(AwaitError::Transport)?;
        if reply.get("ok").as_bool() != Some(true) {
            return Err(AwaitError::BadReply(json::stringify(reply.get("error"))));
        }
        Ok(reply
            .get("lines")
            .as_arr()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|line| line.as_str().map(str::to_string))
            .collect())
    }
}

/// Pre-run snapshot: only a code/error that DIFFERS from it ends the wait,
/// so a stale daemon marker from the previous run cannot fake a verdict.
pub struct Baseline {
    pub code: Value,
    pub error: Value,
}

impl Baseline {
    pub fn capture(state: &Value) -> Self {
        Baseline { code: state.get("last_code").clone(), error: state.get("last_error").clone() }
    }

    pub fn empty() -> Self {
        Baseline { code: Value::Null, error: Value::Null }
    }
}

fn same_json(left: &Value, right: &Value) -> bool {
    json::stringify(left) == json::stringify(right)
}

fn short_error(error: &Value) -> String {
    const LIMIT: usize = 200;
    let text = json::stringify(error);
    if text.len() <= LIMIT { text } else { format!("{}...", &text[..LIMIT]) }
}

fn verdict_by_code(code: &Value, error: &Value) -> (Verdict, String) {
    match code.as_i64() {
        Some(0) => (Verdict::Ok, String::new()),
        Some(exit) if exit == i64::from(lifescape_scripts::kit::cycle::BUDGET_EXIT) => {
            (Verdict::Budget, String::new())
        }
        Some(exit) => (Verdict::Fail, format!("exit {exit}")),
        None => (Verdict::Fail, short_error(error)),
    }
}

/// The terminal verdict for one polled state, if any. Fresh markers win
/// over a stale `running` status (W5); status/phase arbitrate the rest.
pub fn terminal(state: &Value, baseline: &Baseline, saw_running: bool) -> Option<(Verdict, String)> {
    let code = state.get("last_code");
    let error = state.get("last_error");
    let status = state.get("status").as_str().unwrap_or("");
    if !code.is_null() && !same_json(code, &baseline.code) {
        return Some(verdict_by_code(code, error));
    }
    if !error.is_null() && !same_json(error, &baseline.error) {
        return Some((Verdict::Fail, short_error(error)));
    }
    match status {
        "finished" => {
            return Some(if code.is_null() {
                (Verdict::Ok, "finished without code".to_string())
            } else {
                verdict_by_code(code, error)
            });
        }
        "failed" => {
            return Some(if code.as_i64() == Some(i64::from(lifescape_scripts::kit::cycle::BUDGET_EXIT)) {
                (Verdict::Budget, String::new())
            } else {
                (Verdict::Fail, short_error(error))
            });
        }
        _ => {}
    }
    // Unloaded after the end (the supervisor auto-idles `finished`):
    // markers decide, their absence is a fail, never a silent ok.
    if saw_running {
        let script_gone = state.get("script").is_null();
        let phase = state.get("phase").as_str().unwrap_or("");
        let parked = phase == "idle"
            || ((status == "idle" || status == "disconnected") && script_gone);
        if parked {
            if !code.is_null() {
                return Some(verdict_by_code(code, error));
            }
            if !error.is_null() {
                return Some((Verdict::Fail, short_error(error)));
            }
            return Some((Verdict::Fail, "ended without code (unloaded?)".to_string()));
        }
    }
    None
}

fn is_active(state: &Value) -> bool {
    matches!(state.get("status").as_str().unwrap_or(""), "running" | "busy" | "reconnecting" | "starting")
}

#[derive(Debug)]
pub enum WaitEnd {
    Done(Verdict, String),
    NoSession,
    WallTimeout,
}

/// Poll until `terminal` fires or the wall budget dies. `advance` paces
/// iterations (a 300 ms sleep live, a fake-clock step in tests).
pub fn wait_for_end(
    source: &mut dyn StateSource,
    baseline: &Baseline,
    start_wall: f64,
    timeout_s: f64,
    saw_running: bool,
    now: &dyn Fn() -> f64,
    advance: &dyn Fn(),
) -> (WaitEnd, Value) {
    let mut seen = saw_running;
    let mut failures = 0;
    let mut last = Value::Null;
    loop {
        if now() - start_wall >= timeout_s {
            return (WaitEnd::WallTimeout, last);
        }
        match source.show() {
            Err(AwaitError::NoSession(_)) => return (WaitEnd::NoSession, last),
            Err(AwaitError::BadReply(detail)) => return (WaitEnd::Done(Verdict::Fail, detail), last),
            Err(AwaitError::Transport(detail)) => {
                failures += 1;
                if failures >= MAX_TRANSPORT_ERRORS {
                    return (WaitEnd::Done(Verdict::Fail, format!("session unreachable: {detail}")), last);
                }
            }
            Ok(state) => {
                failures = 0;
                if is_active(&state) {
                    seen = true;
                }
                if let Some((verdict, detail)) = terminal(&state, baseline, seen) {
                    return (WaitEnd::Done(verdict, detail), state);
                }
                last = state;
            }
        }
        advance();
    }
}

pub struct AwaitRequest {
    pub account: String,
    pub script: Option<String>,
    pub timeout_s: f64,
    pub json: bool,
}

fn ticks_of(state: &Value) -> Option<i64> {
    state.get("ticks").as_i64()
}

fn pos_of(state: &Value) -> Option<(i64, i64, i64)> {
    let pos = state.get("position").as_arr().cloned().unwrap_or_default();
    if pos.len() >= 3 {
        match (pos[0].as_i64(), pos[1].as_i64(), pos[2].as_i64()) {
            (Some(level), Some(x), Some(z)) => Some((level, x, z)),
            _ => None,
        }
    } else {
        None
    }
}

/// Single human line: every §70 field, then the log tail verbatim below.
pub fn render_human(
    account: &str,
    script: &str,
    verdict: Verdict,
    detail: &str,
    state: &Value,
    start_ticks: Option<i64>,
    wall_s: f64,
) -> String {
    let status = state.get("status").as_str().unwrap_or("-");
    let code = state.get("last_code").as_i64().map(|code| code.to_string()).unwrap_or_else(|| "-".to_string());
    let mut error = if state.get("last_error").is_null() { "-".to_string() } else { short_error(state.get("last_error")) };
    if error == "-" && !detail.is_empty() {
        error = detail.to_string();
    }
    let ticks = ticks_of(state).map(|ticks| ticks.to_string()).unwrap_or_else(|| "-".to_string());
    let ms_per_tick = match (start_ticks, ticks_of(state)) {
        (Some(first), Some(last)) => format!("{:.1}", wall_s * 1000.0 / i64::max(1, last.saturating_sub(first)) as f64),
        _ => "-".to_string(),
    };
    let pos = pos_of(state).map(|(level, x, z)| format!("{level},{x},{z}")).unwrap_or_else(|| "-".to_string());
    format!(
        "VERDICT account={account} script={script} result={} status={status} code={code} error={error} ticks={ticks} wall_s={wall_s:.1} ms_per_tick={ms_per_tick} pos={pos}",
        verdict.name(),
    )
}

pub fn render_json(
    account: &str,
    script: &str,
    verdict: Verdict,
    detail: &str,
    state: &Value,
    start_ticks: Option<i64>,
    wall_s: f64,
    logs: &[String],
) -> String {
    let ms_per_tick = match (start_ticks, ticks_of(state)) {
        (Some(first), Some(last)) => {
            Value::Float(wall_s * 1000.0 / i64::max(1, last.saturating_sub(first)) as f64)
        }
        _ => Value::Null,
    };
    let error = if state.get("last_error").is_null() {
        if detail.is_empty() { Value::Null } else { str_value(detail) }
    } else {
        state.get("last_error").clone()
    };
    json::stringify(&object(vec![
        ("verdict", str_value(verdict.name())),
        ("account", str_value(account)),
        ("script", str_value(script)),
        ("status", str_value(state.get("status").as_str().unwrap_or(""))),
        ("code", state.get("last_code").clone()),
        ("error", error),
        ("ticks", state.get("ticks").clone()),
        ("wall_s", Value::Float((wall_s * 10.0).round() / 10.0)),
        ("ms_per_tick", ms_per_tick),
        ("position", state.get("position").clone()),
        ("detail", str_value(detail)),
        ("logs", Value::Arr(logs.iter().map(|line| str_value(line)).collect())),
    ]))
}

fn finish(
    source: &mut dyn StateSource,
    account: &str,
    script_label: &str,
    end: WaitEnd,
    state: &Value,
    start_ticks: Option<i64>,
    start_wall: f64,
    json: bool,
) -> i32 {
    let wall_s = crate::jobs::now_secs() - start_wall;
    let (verdict, detail) = match end {
        WaitEnd::Done(verdict, detail) => (verdict, detail),
        WaitEnd::NoSession => (Verdict::NoSession, format!("no session, open {account} first")),
        WaitEnd::WallTimeout => (Verdict::WallTimeout, "wall timeout".to_string()),
    };
    let logs = source.logs(LOG_TAIL).unwrap_or_default();
    if json {
        println!("{}", render_json(account, script_label, verdict, &detail, state, start_ticks, wall_s, &logs));
    } else {
        println!("{}", render_human(account, script_label, verdict, &detail, state, start_ticks, wall_s));
        for line in &logs {
            println!("{line}");
        }
    }
    verdict.code()
}

/// CLI entry for `await <compte> [--timeout-s N] [--json]`: parse, then block.
pub fn await_cli(account: &str, rest: &[String]) -> i32 {
    if account.is_empty() {
        eprintln!("usage: orchestrator await <compte> [--timeout-s N] [--json]");
        return 2;
    }
    let (extra, flags) = match split_flags(rest) {
        Ok(split) => split,
        Err(error) => {
            eprintln!("await: {error}");
            return 2;
        }
    };
    if !extra.is_empty() {
        eprintln!("await: unexpected args {extra:?} (usage: orchestrator await <compte> [--timeout-s N] [--json])");
        return 2;
    }
    await_account(&AwaitRequest { account: account.to_string(), script: None, timeout_s: flags.timeout_s, json: flags.json })
}

/// CLI entry for `run`: `Some(exit)` when `--wait` was passed (already
/// waited), `None` for the classic fire-and-forget path. Unknown `--*`
/// only fails in `--wait` mode; without it every arg stays a script arg.
pub fn run_if_wait(account: &str, script: &str, raw_args: &[String]) -> Option<i32> {
    if !raw_args.iter().any(|arg| arg == "--wait") {
        return None;
    }
    let (script_args, flags) = match split_flags(raw_args) {
        Ok(split) => split,
        Err(error) => {
            eprintln!("run: {error}");
            return Some(2);
        }
    };
    Some(run_and_wait(account, script, &script_args, flags.timeout_s, flags.json))
}

/// `orchestrator await <compte>`: block on the current run, if any.
pub fn await_account(request: &AwaitRequest) -> i32 {
    let start_wall = crate::jobs::now_secs();
    let mut source = DaemonSource { account: request.account.clone() };
    let first = match source.show() {
        Err(AwaitError::NoSession(_)) => {
            return finish(
                &mut source,
                &request.account,
                request.script.as_deref().unwrap_or("-"),
                WaitEnd::NoSession,
                &Value::Null,
                None,
                start_wall,
                request.json,
            );
        }
        Err(error) => {
            eprintln!("await: {error}");
            return 1;
        }
        Ok(state) => state,
    };
    // Nothing running and markers already settled: report them now
    // instead of polling a dead run into a wall-timeout.
    if !is_active(&first) {
        let baseline = Baseline::empty();
        if let Some((verdict, detail)) = terminal(&first, &baseline, false) {
            let script = request
                .script
                .clone()
                .or_else(|| first.get("script").as_str().map(str::to_string))
                .unwrap_or_else(|| "-".to_string());
            return finish(&mut source, &request.account, &script, WaitEnd::Done(verdict, detail), &first, ticks_of(&first), start_wall, request.json);
        }
        if !first.get("last_code").is_null() || !first.get("last_error").is_null() {
            let script = request
                .script
                .clone()
                .or_else(|| first.get("script").as_str().map(str::to_string))
                .unwrap_or_else(|| "-".to_string());
            let (verdict, detail) = verdict_by_code(first.get("last_code"), first.get("last_error"));
            return finish(&mut source, &request.account, &script, WaitEnd::Done(verdict, detail), &first, ticks_of(&first), start_wall, request.json);
        }
        eprintln!("await: nothing running on {}", request.account);
        return 1;
    }
    let script = request
        .script
        .clone()
        .or_else(|| first.get("script").as_str().map(str::to_string))
        .unwrap_or_else(|| "-".to_string());
    let baseline = Baseline::capture(&first);
    let start_ticks = ticks_of(&first);
    let advance = || std::thread::sleep(Duration::from_millis(POLL_MS));
    let (end, last) = wait_for_end(&mut source, &baseline, start_wall, request.timeout_s, true, &crate::jobs::now_secs, &advance);
    let state = if last.is_null() { first } else { last };
    finish(&mut source, &request.account, &script, end, &state, start_ticks, start_wall, request.json)
}

/// `orchestrator run <compte> <script> [args] --wait`: start, then block.
pub fn run_and_wait(
    account: &str,
    script: &str,
    script_args: &[String],
    timeout_s: f64,
    json: bool,
) -> i32 {
    let start_wall = crate::jobs::now_secs();
    let mut source = DaemonSource { account: account.to_string() };
    let baseline = match source.show() {
        Ok(state) => Baseline::capture(&state),
        Err(_) => Baseline::empty(),
    };
    let reply = crate::cli::send_command(
        &object(vec![
            ("cmd", str_value("run")),
            ("account", str_value(account)),
            ("script", str_value(script)),
            ("args", Value::Arr(script_args.iter().map(|arg| str_value(arg)).collect())),
        ]),
        Duration::from_secs(crate::daemon::WORKER_TIMEOUT),
    );
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("session unreachable: {error}");
            return 1;
        }
    };
    if reply.get("ok").as_bool() != Some(true) {
        let error = reply.get("error").as_str().unwrap_or("run refused").to_string();
        if error.contains("no session") {
            return finish(&mut source, account, script, WaitEnd::NoSession, &Value::Null, None, start_wall, json);
        }
        println!("{}", json::stringify_pretty(&reply));
        return 1;
    }
    let start_ticks = reply.get("session").get("ticks").as_i64();
    let advance = || std::thread::sleep(Duration::from_millis(POLL_MS));
    let (end, last) = wait_for_end(&mut source, &baseline, start_wall, timeout_s, true, &crate::jobs::now_secs, &advance);
    finish(&mut source, account, script, end, &last, start_ticks, start_wall, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    fn state(status: &str, code: Value, error: Value) -> Value {
        object(vec![
            ("account", str_value("t01")),
            ("script", str_value("probe")),
            ("phase", str_value(if status == "running" { "running" } else { "idle" })),
            ("status", str_value(status)),
            ("ticks", Value::Int(100)),
            (
                "position",
                Value::Arr(vec![Value::Int(0), Value::Int(3222), Value::Int(3222)]),
            ),
            ("last_code", code),
            ("last_error", error),
        ])
    }

    struct Scripted {
        shows: VecDeque<Result<Value, String>>,
    }

    impl StateSource for Scripted {
        fn show(&mut self) -> Result<Value, AwaitError> {
            self.shows.pop_front().unwrap_or(Ok(state("running", Value::Null, Value::Null))).map_err(|kind| {
                if kind == "no-session" {
                    AwaitError::NoSession("gone".to_string())
                } else {
                    AwaitError::Transport(kind)
                }
            })
        }

        fn logs(&mut self, _lines: usize) -> Result<Vec<String>, AwaitError> {
            Ok(vec!["log line".to_string()])
        }
    }

    /// Fake clock: each iteration steps 0.3 s, no real sleeping.
    fn harness(shows: Vec<Result<Value, String>>) -> (Scripted, Rc<Cell<f64>>) {
        let clock = Rc::new(Cell::new(1000.0));
        (Scripted { shows: shows.into_iter().collect() }, clock)
    }

    fn run_wait(shows: Vec<Result<Value, String>>, timeout_s: f64) -> (WaitEnd, Value) {
        let (mut source, clock) = harness(shows);
        let now = {
            let clock = clock.clone();
            move || clock.get()
        };
        let advance = {
            let clock = clock.clone();
            move || clock.set(clock.get() + 0.3)
        };
        wait_for_end(&mut source, &Baseline::empty(), 1000.0, timeout_s, false, &now, &advance)
    }

    /// W5: status stuck at `running` must not hide a fresh exit code.
    #[test]
    fn stale_running_status_yields_to_fresh_code() {
        let running = || Ok(state("running", Value::Null, Value::Null));
        let (end, _) = run_wait(vec![running(), running(), Ok(state("running", Value::Int(0), Value::Null))], 30.0);
        match end {
            WaitEnd::Done(Verdict::Ok, _) => {}
            other => panic!("expected ok, got {other:?}"),
        }
    }

    /// Budget exit (2) is its own verdict, not a failure.
    #[test]
    fn budget_code_maps_to_budget() {
        let (end, _) = run_wait(vec![Ok(state("running", Value::Int(2), Value::Null))], 30.0);
        match end {
            WaitEnd::Done(Verdict::Budget, _) => assert_eq!(Verdict::Budget.code(), 2),
            other => panic!("expected budget, got {other:?}"),
        }
    }

    /// W5 échec: failure with a null code verdicts on `last_error` alone.
    #[test]
    fn error_without_code_is_fail() {
        let error = object(vec![("message", str_value("boom"))]);
        let (end, _) = run_wait(vec![Ok(state("running", Value::Null, error))], 30.0);
        match end {
            WaitEnd::Done(Verdict::Fail, detail) => assert!(detail.contains("boom"), "detail kept: {detail}"),
            other => panic!("expected fail, got {other:?}"),
        }
    }

    /// Same code twice (previous run also 0): the status flip still ends it.
    #[test]
    fn same_code_twice_caught_by_status_flip() {
        let baseline = Baseline { code: Value::Int(0), error: Value::Null };
        let (mut source, clock) = harness(vec![Ok(state("finished", Value::Int(0), Value::Null))]);
        let now = {
            let clock = clock.clone();
            move || clock.get()
        };
        let advance = {
            let clock = clock.clone();
            move || clock.set(clock.get() + 0.3)
        };
        let (end, _) = wait_for_end(&mut source, &baseline, 1000.0, 30.0, true, &now, &advance);
        match end {
            WaitEnd::Done(Verdict::Ok, _) => {}
            other => panic!("expected ok via status, got {other:?}"),
        }
    }

    /// A run that never ends hits the wall budget: verdict + exit 124.
    #[test]
    fn endless_run_hits_wall_timeout() {
        let running = || Ok(state("running", Value::Null, Value::Null));
        let (end, _) = run_wait(vec![running(), running(), running(), running(), running()], 1.0);
        match end {
            WaitEnd::WallTimeout => assert_eq!(Verdict::WallTimeout.code(), 124),
            other => panic!("expected wall-timeout, got {other:?}"),
        }
    }

    /// Closed mid-run: no-session verdict, exit 3.
    #[test]
    fn closed_session_maps_to_no_session() {
        let (end, _) = run_wait(vec![Err("no-session".to_string())], 30.0);
        match end {
            WaitEnd::NoSession => assert_eq!(Verdict::NoSession.code(), 3),
            other => panic!("expected no-session, got {other:?}"),
        }
    }

    /// A dead daemon fails fast (5 transport errors), never hangs.
    #[test]
    fn transport_errors_become_fail() {
        let down = || Err::<Value, String>("refused".to_string());
        let (end, _) = run_wait(vec![down(), down(), down(), down(), down(), down()], 300.0);
        match end {
            WaitEnd::Done(Verdict::Fail, detail) => {
                assert_eq!(Verdict::Fail.code(), 1);
                assert!(detail.contains("unreachable"), "loud cause: {detail}");
            }
            other => panic!("expected fail, got {other:?}"),
        }
    }

    /// Flag parsing: only the three known flags are consumed, in any form.
    #[test]
    fn flags_split_from_script_args() {
        let args = vec!["10".to_string(), "--wait".to_string(), "--timeout-s".to_string(), "30".to_string(), "--json".to_string()];
        let (kept, flags) = split_flags(&args).expect("splits");
        assert_eq!(kept, vec!["10".to_string()]);
        assert!(flags.wait && flags.json);
        assert_eq!(flags.timeout_s, 30.0);
        let (kept, flags) = split_flags(&["--timeout-s=12.5".to_string()]).expect("equals form");
        assert!(kept.is_empty());
        assert_eq!(flags.timeout_s, 12.5);
        assert!(split_flags(&["--bogus".to_string()]).is_err(), "unknown --flag fails");
        assert!(split_flags(&["--timeout-s".to_string(), "zero".to_string()]).is_err(), "bad value fails");
        assert!(split_flags(&["--timeout-s".to_string(), "-5".to_string()]).is_err(), "negative fails");
    }

    /// All five verdicts carry their §70 exit code.
    #[test]
    fn verdict_codes_cover_all_five() {
        assert_eq!(
            [Verdict::Ok, Verdict::Budget, Verdict::Fail, Verdict::WallTimeout, Verdict::NoSession]
                .iter()
                .map(|verdict| verdict.code())
                .collect::<Vec<_>>(),
            vec![0, 2, 1, 124, 3],
        );
    }

    /// Both renderers carry every field; the JSON parses and holds 15 logs.
    #[test]
    fn renderers_carry_all_fields() {
        let settled = state("finished", Value::Int(0), Value::Null);
        let logs: Vec<String> = (0..15).map(|index| format!("line {index}")).collect();
        let human = render_human("t01", "probe", Verdict::Ok, "", &settled, Some(90), 3.0);
        for field in ["account=t01", "script=probe", "result=ok", "status=finished", "code=0", "ticks=100", "pos=0,3222,3222"] {
            assert!(human.contains(field), "missing {field} in:\n{human}");
        }
        let parsed = json::parse(&render_json("t01", "probe", Verdict::Ok, "", &settled, Some(90), 3.0, &logs)).expect("json parses");
        assert_eq!(parsed.get("verdict").as_str(), Some("ok"));
        assert_eq!(parsed.get("logs").as_arr().map(|logs| logs.len()), Some(15));
    }
}
