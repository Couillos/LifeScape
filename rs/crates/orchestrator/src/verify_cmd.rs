//! `orchestrator verify`: run declarative live scenarios, one verdict each.
//!
//! A scenario is one JSON file in `scenarios/` (read with `lifescape-json`):
//! `id`, admitted `accounts`, `setup[]` (staff cheats through `admin`),
//! `run` (script, args, tick budget), `assert[]` (checked against a
//! stabilized pre-run baseline), wall `timeout_s`. Each scenario runs on a
//! distinct account; `verify [ids|--all] [--parallel N]` prints one table
//! row per scenario and the failure dossier (cause, position, log tail)
//! only for failures. `--parallel` is clamped to 1..=20 (D38) and to the
//! daemon admission headroom.
//!
//! Setup never trusts a cheat reply: `give` answers `Ok` on a full pack
//! (27/28 seen live), so the runner polls `show` until the effect is
//! visible (10 s max, snapshots lag 8-10 s after a cheat) and the final
//! asserts re-check the observable state, never the exit code.

#![forbid(unsafe_code)]

use std::time::Duration;

use lifescape_json::{self as json, Value, object, str_value};

use crate::await_cmd::{AwaitError, Baseline, DaemonSource, StateSource, Verdict, WaitEnd, POLL_MS, terminal};
use crate::daemon::WORKER_TIMEOUT;
use crate::verify_assert::{check_assert, position_of};
use crate::verify_reject::{RejectWatch, max_msg_tick};
use crate::verify_scenario::{Scenario, SetupOp, list_scenarios, load_scenario};

const SETUP_WAIT_S: f64 = 10.0;
const LOG_TAIL: usize = 10;
/// D38: at most 20 live bots; `verify` never asks for more at once.
const MAX_PARALLEL: usize = 20;

fn daemon_send(request: &Value, timeout: Duration) -> Result<Value, String> {
    crate::cli::send_command(request, timeout)
        .map_err(|error| format!("session unreachable: {error}"))
}

fn show_of(account: &str) -> Result<Value, String> {
    let reply = daemon_send(
        &object(vec![("cmd", str_value("show")), ("account", str_value(account))]),
        Duration::from_secs(10),
    )?;
    if reply.get("ok").as_bool() != Some(true) {
        return Err(json::stringify(reply.get("error")));
    }
    Ok(reply.get("state").clone())
}

fn is_live(state: &Value) -> bool {
    state.as_obj().is_some() && !state.get("account").as_str().unwrap_or("").is_empty()
}

/// Spectate shows the game, not the wire: over-fetch the tail, drop
/// packet lines and machine JSON reports, keep the operator signal.
fn log_tail(account: &str, lines: usize) -> Vec<String> {
    let reply = daemon_send(
        &object(vec![
            ("cmd", str_value("logs")),
            ("account", str_value(account)),
            ("lines", Value::Int((lines * 2) as i64)),
        ]),
        Duration::from_secs(10),
    )
    .unwrap_or(Value::Null);
    reply
        .get("lines")
        .as_arr()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|line| line.as_str().map(str::to_string))
        .filter(|line| !line.contains("task_report"))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// -- live run ----------------------------------------------------------------

struct Outcome {
    id: String,
    account: String,
    verdict: Verdict,
    detail: String,
    ticks: Option<i64>,
    elapsed: Option<i64>,
    wall_s: f64,
    run_s: f64,
    dossier: Vec<String>,
}

fn cheat_on_admin(cheat: &str) -> Result<(), String> {
    let reply = daemon_send(
        &object(vec![
            ("cmd", str_value("call")),
            ("account", str_value("admin")),
            ("method", str_value("cheat")),
            ("args", Value::Arr(vec![str_value(cheat)])),
        ]),
        Duration::from_secs(WORKER_TIMEOUT),
    )?;
    if reply.get("ok").as_bool() == Some(true) {
        Ok(())
    } else {
        Err(json::stringify(reply.get("error")))
    }
}

/// A live worker, not a stopped file: `show` serves both, `status`
/// only lists attached workers.
fn worker_live(account: &str) -> bool {
    let reply = daemon_send(
        &object(vec![("cmd", str_value("status")), ("account", str_value(account))]),
        Duration::from_secs(10),
    )
    .unwrap_or(Value::Null);
    reply.get("jobs").as_arr().map(|jobs| !jobs.is_empty()).unwrap_or(false)
}

fn ensure_open(account: &str) -> Result<(), String> {
    // Live workers need no open: the daemon paces accepts (E10, one per
    // 2 s), so a redundant `open` inside the window reads as a refusal.
    if worker_live(account) {
        return Ok(());
    }
    for _attempt in 0..5 {
        let reply = daemon_send(
            &object(vec![("cmd", str_value("open")), ("account", str_value(account))]),
            Duration::from_secs(WORKER_TIMEOUT),
        )?;
        if reply.get("ok").as_bool() == Some(true) {
            return Ok(());
        }
        let error = reply.get("error").as_str().unwrap_or("open refused").to_string();
        if error.contains("already open") {
            return Ok(());
        }
        if error.contains("admission:open-rate") {
            std::thread::sleep(Duration::from_millis(2500));
            continue;
        }
        return Err(error);
    }
    Err(format!("open {account}: paced out by admission:open-rate"))
}

fn short_error(error: &Value) -> String {
    const LIMIT: usize = 160;
    let text = json::stringify(error);
    if text.len() <= LIMIT { text } else { format!("{}...", &text[..LIMIT]) }
}

/// Poll `cond` until it sees the wanted state or the budget dies. Fresh
/// snapshots lag the cheat by seconds: 1 s cadence, never a blind sleep.
/// Returns the last state and whether the condition was ever met: a cheat
/// that answers Ok on a stale or busy session (full pack, mid-login)
/// must fail the setup loudly instead of launching a doomed run.
fn poll_until(_description: &str, account: &str, budget_s: f64, cond: &dyn Fn(&Value) -> bool) -> (Option<Value>, bool) {
    let start = crate::jobs::now_secs();
    loop {
        match show_of(account) {
            Ok(state) if is_live(&state) && cond(&state) => return (Some(state), true),
            Ok(state) if is_live(&state) => {
                if crate::jobs::now_secs() - start >= budget_s {
                    return (Some(state), false);
                }
            }
            _ => {
                if crate::jobs::now_secs() - start >= budget_s {
                    return (None, false);
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn setup_account(scenario: &Scenario, account: &str) -> Result<Value, String> {
    ensure_open("admin")?;
    ensure_open(account)?;
    let hold = daemon_send(
        &object(vec![("cmd", str_value("hold")), ("account", str_value(account))]),
        Duration::from_secs(WORKER_TIMEOUT),
    )?;
    if hold.get("ok").as_bool() != Some(true) {
        return Err(format!("hold {account}: {}", json::stringify(hold.get("error"))));
    }
    let before = show_of(account).unwrap_or(Value::Null);
    // A fresh worker may still be logging in: no ticks, no setup. The
    // cheats would answer Ok against a ghost session and silently miss.
    let before_ticks = before.get("ticks").as_i64();
    let (_, streaming) = poll_until("streaming", account, 20.0, &|state| {
        state.get("ticks").as_i64().is_some_and(|ticks| Some(ticks) != before_ticks)
    });
    if !streaming {
        return Err(format!("{account}: session not streaming, open it again"));
    }
    let mut admin_pos = show_of("admin").ok().and_then(|state| position_of(&state));
    for op in &scenario.setup {
        match op {
            SetupOp::TeleAdmin { level, x, z } => {
                let cheat = format!("tele {level},{},{},{},{}", x >> 6, z >> 6, x & 63, z & 63);
                cheat_on_admin(&cheat)?;
                let want = (*level, *x, *z);
                admin_pos = poll_until("tele_admin", "admin", SETUP_WAIT_S, &|state| position_of(&state) == Some(want))
                    .0
                    .and_then(|state| position_of(&state));
            }
            SetupOp::Teleother => {
                cheat_on_admin(&format!("teleother {account}"))?;
                if let Some(want) = admin_pos {
                    let (_, landed) = poll_until("teleother", account, SETUP_WAIT_S, &|state| position_of(&state) == Some(want));
                    if !landed {
                        return Err(format!("teleother {account}: target never at ({},{},{})", want.0, want.1, want.2));
                    }
                } else {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            SetupOp::BankAll { keep } => {
                // Hermetic pack: the `bank_deposit` devtool (never game
                // logic) empties the pack so the run below harvests fresh
                // goods instead of re-banking leftovers. Runs in order:
                // authors list it first in the setup.
                let pre = show_of(account).map_err(|error| format!("show {account}: {error}"))?;
                let reply = daemon_send(
                    &object(vec![
                        ("cmd", str_value("run")),
                        ("account", str_value(account)),
                        ("script", str_value("bank_deposit")),
                        ("args", Value::Arr(keep.iter().map(|name| str_value(name)).collect())),
                    ]),
                    Duration::from_secs(WORKER_TIMEOUT),
                )
                .map_err(|error| format!("bank_all run: {error}"))?;
                if reply.get("ok").as_bool() != Some(true) {
                    return Err(format!("bank_all run refused: {}", json::stringify(reply.get("error"))));
                }
                let start = crate::jobs::now_secs();
                let (end, _) = watch_run(account, &Baseline::capture(&pre), max_msg_tick(&pre), pre.get("ticks").as_i64(), start, 300.0, 3000);
                match end {
                    WaitEnd::Done(Verdict::Ok, _) => {}
                    WaitEnd::Done(other, detail) => return Err(format!("bank_all: {result} {detail}", result = other.name())),
                    WaitEnd::NoSession => return Err(format!("bank_all: no session, open {account} first")),
                    WaitEnd::WallTimeout => return Err("bank_all: wall timeout".to_string()),
                }
            }
            SetupOp::Setvarother { var, value } => {
                cheat_on_admin(&format!("setvarother {account} {var} {value}"))?;
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
    // Stabilized baseline: one last poll so the asserts compare against
    // what the runner really saw, not a stale snapshot.
    std::thread::sleep(Duration::from_secs(2));
    show_of(account).map_err(|error| format!("show {account}: {error}"))
}

fn unload(account: &str) {
    let _ = daemon_send(
        &object(vec![("cmd", str_value("unload")), ("account", str_value(account))]),
        Duration::from_secs(WORKER_TIMEOUT),
    );
}

fn watch_run(account: &str, baseline: &Baseline, baseline_msg: i64, start_ticks: Option<i64>, start_wall: f64, timeout_s: f64, budget_ticks: i64) -> (WaitEnd, Value) {
    let mut source = DaemonSource { account: account.to_string() };
    let mut failures = 0;
    let mut rejects = RejectWatch::new();
    let mut last = Value::Null;
    let saw_running = true;
    loop {
        let now = crate::jobs::now_secs();
        if now - start_wall >= timeout_s {
            unload(account);
            return (WaitEnd::WallTimeout, last);
        }
        match source.show() {
            Err(AwaitError::NoSession(_)) => return (WaitEnd::NoSession, last),
            Err(AwaitError::BadReply(detail)) => {
                return (WaitEnd::Done(Verdict::Fail, detail), last);
            }
            Err(AwaitError::Transport(detail)) => {
                failures += 1;
                if failures >= 5 {
                    return (WaitEnd::Done(Verdict::Fail, format!("session unreachable: {detail}")), last);
                }
            }
            Ok(state) => {
                failures = 0;
                if let Some(ticks) = state.get("ticks").as_i64() {
                    if let Some(first) = start_ticks {
                        if ticks.saturating_sub(first) >= budget_ticks {
                            unload(account);
                            return (
                                WaitEnd::Done(Verdict::Budget, format!("budget {budget_ticks} ticks")),
                                state,
                            );
                        }
                    }
                }
                if let Some((verdict, detail)) = terminal(&state, &baseline, saw_running) {
                    return (WaitEnd::Done(verdict, detail), state);
                }
                // W4: an explicit server rejection on a stuck bot ends the
                // run here with its cause, never at the budget.
                if let Some(cause) = rejects.note(&state, baseline_msg, &baseline.error) {
                    unload(account);
                    return (WaitEnd::Done(Verdict::Fail, cause), state);
                }
                last = state;
            }
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn render_dossier(outcome: &Outcome, script: &str, baseline: &Value, state: &Value, failed_assert: Option<&str>, logs: &[String]) -> Vec<String> {
    let code = state.get("last_code").as_i64().map(|code| code.to_string()).unwrap_or_else(|| "-".to_string());
    // The daemon keeps the last error ever (sticky nulls): an error
    // identical to the pre-run baseline belongs to an older run, not this one.
    let stale = !state.get("last_error").is_null() && json::stringify(state.get("last_error")) == json::stringify(baseline.get("last_error"));
    let mut error = if state.get("last_error").is_null() || stale {
        "-".to_string()
    } else {
        short_error(state.get("last_error"))
    };
    if error == "-" && !outcome.detail.is_empty() {
        error = outcome.detail.clone();
    }
    let ticks = state.get("ticks").as_i64().map(|ticks| ticks.to_string()).unwrap_or_else(|| "-".to_string());
    let pos = position_of(state).map(|(level, x, z)| format!("{level},{x},{z}")).unwrap_or_else(|| "-".to_string());
    let ms_per_tick = match (outcome.ticks, state.get("ticks").as_i64()) {
        (Some(first), Some(last)) => format!("{:.1}", outcome.run_s * 1000.0 / i64::max(1, last.saturating_sub(first)) as f64),
        _ => "-".to_string(),
    };
    let mut lines = vec![
        format!("FAIL {} on {}: {}", outcome.id, outcome.account, outcome.detail),
        format!("script={script} result={} code={code} error={error}", outcome.verdict.name()),
        format!("ticks={ticks} wall_s={wall:.1} ms_per_tick={ms_per_tick} pos={pos}", wall = outcome.wall_s),
    ];
    if let Some(assert) = failed_assert {
        lines.push(format!("assert: {assert}"));
    }
    for line in logs.iter().take(LOG_TAIL) {
        lines.push(format!("log: {line}"));
    }
    lines
}

/// Phase 1 (always serial: one `admin` drives every cheat, the positions
/// would race otherwise). Returns the stabilized baseline, or the setup
/// failure outcome when the start state never became visible.
fn setup_single(scenario: &Scenario, account: &str, wall_start: f64) -> Result<Value, Outcome> {
    let fail = |verdict: Verdict, detail: String, ticks: Option<i64>, dossier: Vec<String>| Outcome {
        id: scenario.id.clone(),
        account: account.to_string(),
        verdict,
        detail,
        ticks,
        elapsed: None,
        wall_s: crate::jobs::now_secs() - wall_start,
        run_s: 0.0,
        dossier,
    };
    match setup_account(scenario, account) {
        Ok(state) if is_live(&state) => Ok(state),
        Ok(_) => Err(fail(Verdict::NoSession, format!("no session, open {account} first"), None, vec![format!("FAIL {} on {account}: no session", scenario.id)])),
        Err(error) => Err(fail(
            Verdict::Fail,
            format!("setup: {error}"),
            None,
            vec![
                format!("FAIL {} on {account}: setup: {error}", scenario.id),
                format!("pos={}", show_of(account).ok().and_then(|state| position_of(&state)).map(|(level, x, z)| format!("{level},{x},{z}")).unwrap_or_else(|| "-".to_string())),
            ],
        )),
    }
}

/// Phase 2 (parallel-safe: the scripts run on distinct accounts, the
/// runner only polls). Dispatches the run, watches it to the end, then
/// checks every assert against the phase-1 baseline.
fn finish_single(scenario: &Scenario, account: &str, baseline: &Value, wall_start: f64) -> Outcome {
    let fail = |verdict: Verdict, detail: String, ticks: Option<i64>, dossier: Vec<String>| Outcome {
        id: scenario.id.clone(),
        account: account.to_string(),
        verdict,
        detail,
        ticks,
        elapsed: None,
        wall_s: crate::jobs::now_secs() - wall_start,
        run_s: 0.0,
        dossier,
    };
    let start_ticks = baseline.get("ticks").as_i64();
    let start_wall = crate::jobs::now_secs();
    let run_args = Value::Arr(scenario.run.args.iter().map(|arg| str_value(arg)).collect());
    let run_reply = daemon_send(
        &object(vec![
            ("cmd", str_value("run")),
            ("account", str_value(account)),
            ("script", str_value(&scenario.run.script)),
            ("args", run_args),
        ]),
        Duration::from_secs(WORKER_TIMEOUT),
    );
    match run_reply {
        Ok(reply) if reply.get("ok").as_bool() == Some(true) => {}
        Ok(reply) => {
            let error = reply.get("error").as_str().unwrap_or("run refused").to_string();
            if error.contains("no session") {
                return fail(Verdict::NoSession, format!("no session, open {account} first"), start_ticks, vec![format!("FAIL {} on {account}: no session", scenario.id)]);
            }
            return fail(Verdict::Fail, format!("run refused: {error}"), start_ticks, vec![format!("FAIL {} on {account}: run refused: {error}", scenario.id)]);
        }
        Err(error) => return fail(Verdict::Fail, error.clone(), start_ticks, vec![format!("FAIL {} on {account}: {error}", scenario.id)]),
    }
    let (end, state) = watch_run(account, &Baseline::capture(&baseline), max_msg_tick(&baseline), start_ticks, start_wall, scenario.timeout_s, scenario.run.budget_ticks);
    let (verdict, detail) = match end {
        WaitEnd::Done(verdict, detail) => (verdict, detail),
        WaitEnd::NoSession => (Verdict::NoSession, format!("no session, open {account} first")),
        WaitEnd::WallTimeout => (Verdict::WallTimeout, "wall timeout".to_string()),
    };
    let state = if state.is_null() { show_of(account).unwrap_or(Value::Null) } else { state };
    let elapsed = match (start_ticks, state.get("ticks").as_i64()) {
        (Some(first), Some(last)) => Some(last.saturating_sub(first).max(0)),
        _ => None,
    };
    let finish = |verdict: Verdict, detail: String, dossier: Vec<String>| Outcome {
        id: scenario.id.clone(),
        account: account.to_string(),
        verdict,
        detail,
        ticks: start_ticks,
        elapsed,
        wall_s: crate::jobs::now_secs() - wall_start,
        run_s: crate::jobs::now_secs() - start_wall,
        dossier,
    };
    if verdict != Verdict::Ok {
        let logs = log_tail(account, LOG_TAIL);
        let mut outcome = finish(verdict, detail.clone(), Vec::new());
        outcome.dossier = render_dossier(&outcome, &scenario.run.script, &baseline, &state, None, &logs);
        return outcome;
    }
    for assert in &scenario.assert {
        if let Err(reason) = check_assert(assert, &baseline, &state) {
            let logs = log_tail(account, LOG_TAIL);
            let mut outcome = finish(Verdict::Fail, reason.clone(), Vec::new());
            outcome.dossier = render_dossier(&outcome, &scenario.run.script, &baseline, &state, Some(&reason), &logs);
            return outcome;
        }
    }
    finish(Verdict::Ok, String::new(), Vec::new())
}

// -- table + parallelism -----------------------------------------------------

fn admission_headroom() -> Option<usize> {
    let status = daemon_send(&object(vec![("cmd", str_value("status"))]), Duration::from_secs(10)).ok()?;
    let online = status.get("jobs").as_arr().map(Vec::len).unwrap_or(0);
    let stats = daemon_send(&object(vec![("cmd", str_value("res-stats"))]), Duration::from_secs(10)).ok()?;
    let load = crate::admission::DaemonLoad {
        online_bots: online,
        pushes_per_s: stats.get("pushes_per_s").as_f64().unwrap_or(0.0),
        push_bytes_per_s: stats.get("push_bytes_per_s").as_f64().unwrap_or(0.0),
        files_per_s: stats.get("files_per_s").as_f64().unwrap_or(0.0),
        plan_submits_per_s: stats.get("plan_submits_per_s").as_f64().unwrap_or(0.0),
    };
    Some(crate::admission::max_online_bots(&load).saturating_sub(online))
}

/// CLI entry: `verify [ids|--all] [--parallel N]`. One distinct account per
/// scenario, a table row each, dossiers for failures only. Exit 0 when
/// every scenario is ok, 1 otherwise, 2 on usage errors.
pub fn verify_cli(args: &[String]) -> i32 {
    let mut ids: Vec<String> = Vec::new();
    let mut all = false;
    let mut parallel = 1usize;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--all" => all = true,
            "--parallel" => {
                index += 1;
                match args.get(index).and_then(|raw| raw.parse::<usize>().ok()).filter(|n| *n >= 1) {
                    Some(n) => parallel = n,
                    None => {
                        eprintln!("verify: --parallel needs a positive number");
                        return 2;
                    }
                }
            }
            flag if flag.starts_with("--") => {
                eprintln!("verify: unknown flag {flag:?} (usage: verify [ids|--all] [--parallel N])");
                return 2;
            }
            id => ids.push(id.to_string()),
        }
        index += 1;
    }
    if all {
        match list_scenarios() {
            Ok(found) => ids = found,
            Err(error) => {
                eprintln!("verify: {error}");
                return 1;
            }
        }
    }
    if ids.is_empty() {
        eprintln!("usage: orchestrator verify [ids|--all] [--parallel N]");
        return 2;
    }
    parallel = parallel.clamp(1, MAX_PARALLEL).min(ids.len());
    if parallel > 1 {
        match admission_headroom() {
            Some(headroom) if headroom < parallel => {
                println!("verify: admission headroom {headroom}, parallel capped");
                parallel = headroom.max(1);
            }
            None => println!("verify: admission unreadable, parallel kept at {parallel}"),
            _ => {}
        }
    }
    let mut scenarios: Vec<Result<Scenario, String>> = Vec::new();
    for id in &ids {
        scenarios.push(load_scenario(id));
    }
    // One distinct account per scenario: first admitted name not taken yet.
    let mut taken: Vec<String> = Vec::new();
    let mut assignments: Vec<(String, Result<Scenario, String>)> = Vec::new();
    for (id, scenario) in ids.iter().zip(scenarios.into_iter()) {
        match scenario {
            Err(error) => assignments.push((String::new(), Err(error.clone()))),
            Ok(scenario) => {
                let free = scenario.accounts.iter().find(|name| !taken.contains(name)).cloned();
                match free {
                    Some(account) => {
                        taken.push(account.clone());
                        assignments.push((account, Ok(scenario)));
                    }
                    None => assignments.push((
                        String::new(),
                        Err(format!("scenario {id}: no free account (admis: {})", scenario.accounts.join(","))),
                    )),
                }
            }
        }
    }
    let load_fail = |error: &str| Outcome {
        id: error.to_string(),
        account: "-".to_string(),
        verdict: Verdict::Fail,
        detail: error.to_string(),
        ticks: None,
        elapsed: None,
        run_s: 0.0,
        wall_s: 0.0,
        dossier: vec![format!("FAIL {error}")],
    };
    let outcomes = if parallel <= 1 {
        assignments
            .into_iter()
            .map(|(account, scenario)| match scenario {
                Ok(scenario) => {
                    let wall_start = crate::jobs::now_secs();
                    match setup_single(&scenario, &account, wall_start) {
                        Ok(baseline) => finish_single(&scenario, &account, &baseline, wall_start),
                        Err(outcome) => outcome,
                    }
                }
                Err(error) => load_fail(&error),
            })
            .collect::<Vec<_>>()
    } else {
        // Phase 1 stays serial: one `admin` positions every account, the
        // cheats would race each other. Phase 2 runs the scripts in
        // parallel on the distinct accounts.
        let mut ready: Vec<Result<(String, Scenario, Value, f64), Outcome>> = Vec::new();
        for (account, scenario) in assignments {
            match scenario {
                Err(error) => ready.push(Err(load_fail(&error))),
                Ok(scenario) => {
                    let wall_start = crate::jobs::now_secs();
                    match setup_single(&scenario, &account, wall_start) {
                        Ok(baseline) => ready.push(Ok((account, scenario, baseline, wall_start))),
                        Err(outcome) => ready.push(Err(outcome)),
                    }
                }
            }
        }
        let mut handles = Vec::new();
        for entry in ready {
            handles.push(std::thread::spawn(move || match entry {
                Ok((account, scenario, baseline, wall_start)) => finish_single(&scenario, &account, &baseline, wall_start),
                Err(outcome) => outcome,
            }));
        }
        let mut outcomes = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(outcome) => outcomes.push(outcome),
                Err(_) => outcomes.push(Outcome {
                    id: "?".to_string(),
                    account: "-".to_string(),
                    verdict: Verdict::Fail,
                    detail: "verify worker panicked".to_string(),
                    ticks: None,
                    elapsed: None,
                    run_s: 0.0,
                    wall_s: 0.0,
                    dossier: vec!["FAIL ?: verify worker panicked".to_string()],
                }),
            }
        }
        outcomes.sort_by(|left, right| left.id.cmp(&right.id));
        outcomes
    };
    println!("verify: {} scenarios, parallel={parallel}", outcomes.len());
    println!("{:<18} {:<10} {:<12} {:<7} {:<7} {}", "id", "account", "result", "ticks", "wall_s", "cause");
    let mut failed = 0;
    for outcome in &outcomes {
        if outcome.verdict != Verdict::Ok {
            failed += 1;
        }
        let ticks = outcome.elapsed.map(|ticks| ticks.to_string()).unwrap_or_else(|| "-".to_string());
        let cause = if outcome.detail.is_empty() { "-" } else { outcome.detail.as_str() };
        println!(
            "{:<18} {:<10} {:<12} {:<7} {:<7.1} {}",
            outcome.id, outcome.account, outcome.verdict.name(), ticks, outcome.wall_s, cause
        );
    }
    for outcome in &outcomes {
        if outcome.verdict != Verdict::Ok {
            for line in &outcome.dossier {
                println!("{line}");
            }
        }
    }
    if failed > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_fixture() -> Value {
        json::parse(
            r#"{
                "account": "t40",
                "position": [0, 3223, 3217],
                "ticks": 120,
                "inv": [{"id": 1, "name": "Wool", "count": 3}],
                "bank": [{"id": 2, "name": "Shrimp", "count": 5}],
                "skills": [{"id": 7, "level": 3, "xp": 130}, {"id": 10, "level": 5, "xp": 600}],
                "messages": ["Welcome to RuneScape.", "You cook the shrimp."],
                "message_ticks": [0, 118],
                "npcs": [{"name": "Sheep", "x": 3222, "z": 3216}]
            }"#,
        )
        .expect("fixture parses")
    }

    fn empty_state() -> Value {
        json::parse(
            r#"{"account": "t40", "position": [0, 3220, 3217], "ticks": 100, "inv": [], "bank": [],
                "skills": [{"id": 7, "level": 3, "xp": 100}], "messages": ["Welcome to RuneScape."], "message_ticks": [0], "npcs": []}"#,
        )
        .expect("fixture parses")
    }

    /// A forced failure fits its dossier in a few lines, cause first.
    #[test]
    fn failure_dossier_stays_short() {
        let outcome = Outcome {
            id: "walk_lumbridge".to_string(),
            account: "t40".to_string(),
            verdict: Verdict::Fail,
            detail: "position: want (0,3223,3217)\u{b1}3, at (0,3220,3217)".to_string(),
            ticks: Some(100),
            elapsed: Some(20),
            wall_s: 9.4,
            run_s: 1.2,
            dossier: Vec::new(),
        };
        let logs: Vec<String> = (0..10).map(|index| format!("line {index}")).collect();
        let dossier = render_dossier(&outcome, "walkto", &empty_state(), &state_fixture(), Some("position: off target"), &logs);
        assert!(dossier.len() < 40, "dossier under 40 lines, got {}", dossier.len());
        assert!(dossier[0].starts_with("FAIL walk_lumbridge"), "cause first: {}", dossier[0]);
        assert!(dossier.iter().any(|line| line.contains("pos=")), "position carried");
    }
}
