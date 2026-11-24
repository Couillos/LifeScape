#![forbid(unsafe_code)]
//! The operator commands of the daemon socket as a typed table (lot 5.3
//! of the architecture plan; split from `daemon.rs`, same replies):
//! `OperatorCommand::parse` reads the request once, `Orchestrator::dispatch`
//! is one arm per command, each arm a handler of its own. The wire stays
//! `{"cmd": ..., "account": ...}` JSON, the CLI and the dashboard are
//! unchanged.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use lifescape_json::{object, str_value, Value};

use crate::daemon::{is_pktlog_line, known_accounts, tail_lines, Orchestrator};
use crate::jobs::{resolve_state, Job};
use crate::paths;
use crate::registry;
use crate::worker_command::WorkerCommand;

/// One request of the daemon socket.
#[derive(Clone, Debug, PartialEq)]
pub enum OperatorCommand {
    /// Needs the daemon's own context (`Arc<Orchestrator>`): answered by the caller.
    Open,
    Run { script: String, args: Vec<Value> },
    Unload,
    Pause,
    Resume,
    Hold,
    Close,
    Call { method: String, args: Vec<Value> },
    Methods,
    Ambition { kind: String },
    Rares,
    FundMakers { dry_run: bool, force: bool },
    BidWhy { item: String },
    Status,
    JobList,
    Items,
    Scripts,
    Accounts,
    Show,
    Live,
    Logs { lines: i64 },
    Shutdown,
    ReloadNav,
    NavStats,
    ResStats,
    BrainWishes,
    Market,
    /// A word this build does not know.
    Unknown(String),
}

impl OperatorCommand {
    pub fn parse(request: &Value) -> OperatorCommand {
        let text = |key: &str| request.get(key).as_str().unwrap_or("").to_string();
        match request.get("cmd").as_str().unwrap_or("") {
            "open" => OperatorCommand::Open,
            "run" => OperatorCommand::Run { script: text("script"), args: request.get("args").as_arr().cloned().unwrap_or_default() },
            "unload" => OperatorCommand::Unload,
            "pause" => OperatorCommand::Pause,
            "resume" => OperatorCommand::Resume,
            "hold" => OperatorCommand::Hold,
            "close" => OperatorCommand::Close,
            // The CLI sends raw strings; the worker parses JSON-or-string.
            "call" => OperatorCommand::Call {
                method: text("method"),
                args: request.get("args").as_arr().cloned().unwrap_or_default().iter().map(|arg| arg.as_str().map(str_value).unwrap_or_else(|| arg.clone())).collect(),
            },
            "methods" => OperatorCommand::Methods,
            "ambition" => OperatorCommand::Ambition { kind: text("kind") },
            "rares" => OperatorCommand::Rares,
            "fund_makers" => OperatorCommand::FundMakers { dry_run: request.get("dry_run").as_bool().unwrap_or(true), force: request.get("force").as_bool().unwrap_or(false) },
            "bid_why" => OperatorCommand::BidWhy { item: text("item") },
            "status" => OperatorCommand::Status,
            "job_list" => OperatorCommand::JobList,
            "items" => OperatorCommand::Items,
            "scripts" => OperatorCommand::Scripts,
            "accounts" => OperatorCommand::Accounts,
            "show" => OperatorCommand::Show,
            "live" => OperatorCommand::Live,
            "logs" => OperatorCommand::Logs { lines: request.get("lines").as_i64().unwrap_or(40) },
            "shutdown" => OperatorCommand::Shutdown,
            "reload-nav" => OperatorCommand::ReloadNav,
            "nav-stats" => OperatorCommand::NavStats,
            "res-stats" => OperatorCommand::ResStats,
            "brain_wishes" => OperatorCommand::BrainWishes,
            "market" => OperatorCommand::Market,
            other => OperatorCommand::Unknown(other.to_string()),
        }
    }
}

fn ok_lines(lines: &[String]) -> Value {
    object(vec![("ok", Value::Bool(true)), ("lines", Value::Arr(lines.iter().map(|line| str_value(line)).collect()))])
}

fn error_reply(error: &str) -> Value {
    object(vec![("ok", Value::Bool(false)), ("error", str_value(error))])
}

impl Orchestrator {
    pub fn dispatch(&self, request: &Value) -> Value {
        let account = request.get("account").as_str().unwrap_or("");
        match OperatorCommand::parse(request) {
            OperatorCommand::Open => error_reply("open needs daemon context"),
            OperatorCommand::Run { script, args } => self.run_job(account, &script, &args),
            OperatorCommand::Unload => {
                crate::autopilot::current().note_operator_stop(account);
                self.simple_worker_cmd(account, WorkerCommand::Unload)
            }
            OperatorCommand::Pause => self.simple_worker_cmd(account, WorkerCommand::Pause),
            OperatorCommand::Resume => self.resume_job(account),
            OperatorCommand::Hold => self.hold_job(account),
            OperatorCommand::Close => {
                crate::autopilot::current().note_operator_stop(account);
                self.close_job(account)
            }
            OperatorCommand::Call { method, args } => self.call_job(account, &method, &args),
            OperatorCommand::Methods => self.methods_job(account),
            OperatorCommand::Ambition { kind } => match crate::brain_operator::request(account, &kind) {
                Ok(()) => object(vec![("ok", Value::Bool(true)), ("pending", str_value(&kind))]),
                Err(error) => error_reply(&error),
            },
            OperatorCommand::Rares => self.rares_reply(),
            OperatorCommand::FundMakers { dry_run, force } => crate::funding_cmd::fund_makers(self, dry_run, force),
            OperatorCommand::BidWhy { item } => match crate::brain_bids::explain_for(account, &item, &lifescape_orch_brain::brain_deps::BrainDeps::live()) {
                Ok(lines) => ok_lines(&lines),
                Err(error) => error_reply(&error),
            },
            OperatorCommand::Status => self.status_reply(account),
            OperatorCommand::JobList => self.job_list_reply(),
            OperatorCommand::Items => {
                let described = crate::holdings::described_jobs(self);
                object(vec![("ok", Value::Bool(true)), ("data", crate::holdings::snapshot(&described))])
            }
            OperatorCommand::Scripts => object(vec![("ok", Value::Bool(true)), ("scripts", Value::Arr(registry::script_names_sorted().iter().map(|n| str_value(n)).collect()))]),
            OperatorCommand::Accounts => self.accounts_reply(),
            OperatorCommand::Show => self.show_reply(account),
            OperatorCommand::Live => self.live_reply(account),
            OperatorCommand::Logs { lines } => logs_reply(account, lines),
            OperatorCommand::Shutdown => {
                self.close_all();
                self.running.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = UnixStream::connect(paths::socket_path());
                object(vec![("ok", Value::Bool(true))])
            }
            // Gamedata changed on disk (overrides, transitions, pack): rebuild
            // the snapshot beside the live one and swap it. In-flight requests
            // finish on the old Arc; the bumped version rejects every stale answer.
            OperatorCommand::ReloadNav => match crate::plan_service::reload() {
                Ok(report) => object(vec![("ok", Value::Bool(true)), ("reloaded", str_value(&report))]),
                Err(error) => error_reply(&error),
            },
            // Lot 4 operator gauge: counters + queue + RAM, no log spam. Down
            // service = explicit error, never stale numbers.
            OperatorCommand::NavStats => crate::plan_service::stats_json().unwrap_or_else(|| error_reply("pathfinding service not running")),
            // Attribution gauge: pushes, bytes, plans, polls, files,
            // connections, each with its per-second average.
            OperatorCommand::ResStats => crate::res_stats::snapshot_json(crate::plan_service::stats_json()),
            // Brain tab of the Lives page: the brain's own wishlist.
            OperatorCommand::BrainWishes => {
                let job = self.jobs.lock().unwrap().get(account).cloned();
                crate::web_brain::wishes_reply(job.as_deref(), account)
            }
            // Market socket API over HTTP: {cmd:"market", op, ...}.
            OperatorCommand::Market => crate::market_service::handle(request),
            OperatorCommand::Unknown(command) => error_reply(&format!("unknown command '{command}'")),
        }
    }

    fn rares_reply(&self) -> Value {
        let accounts: Vec<String> = self.jobs.lock().unwrap().keys().cloned().collect();
        match crate::rares_cmd::rares_lines(&accounts) {
            Ok(lines) => ok_lines(&lines),
            Err(error) => error_reply(&error),
        }
    }

    fn status_reply(&self, account: &str) -> Value {
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
                    map.insert("account_name".to_string(), str_value(&crate::jobs::humanize(job.account.as_str())));
                }
                entry
            })
            .collect();
        crate::jobs::sort_status_rows(&mut described);
        object(vec![("ok", Value::Bool(true)), ("jobs", Value::Arr(described))])
    }

    /// The dashboard's bot list (lot 1.2): `status` sliced to the columns
    /// the list shows, built from those fields only.
    fn job_list_reply(&self) -> Value {
        let live: HashMap<String, Arc<Job>> = self.jobs.lock().unwrap().clone();
        let mut names: Vec<&String> = live.keys().collect();
        names.sort();
        let mut rows: Vec<Value> = names.iter().filter_map(|name| live.get(*name)).map(|job| job.list_row()).collect();
        crate::jobs::sort_status_rows(&mut rows);
        object(vec![("ok", Value::Bool(true)), ("jobs", Value::Arr(rows))])
    }

    fn accounts_reply(&self) -> Value {
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
        object(vec![
            ("ok", Value::Bool(true)),
            ("accounts", Value::Arr(known.iter().map(|n| str_value(n)).collect())),
            ("live", Value::Arr(live.iter().map(|n| str_value(n)).collect())),
        ])
    }

    /// Memory first (pushed snapshots); the files are only the adoption
    /// fallback when no worker is attached.
    fn show_reply(&self, account: &str) -> Value {
        let state = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job.describe(),
            None => {
                let (merged_path, _partial, _log) = paths::user_paths(account);
                let state = paths::read_json(&merged_path).unwrap_or(Value::Null);
                if state.as_obj().is_some() { state } else { object(vec![]) }
            }
        };
        object(vec![("ok", Value::Bool(true)), ("state", resolve_state(state))])
    }

    fn live_reply(&self, account: &str) -> Value {
        let state = match self.jobs.lock().unwrap().get(account).cloned() {
            Some(job) => job.describe(),
            None => {
                let (merged_path, partial_path, _log) = paths::user_paths(account);
                let merged = paths::read_json(&merged_path).unwrap_or(Value::Null);
                let state = match (merged, paths::read_json(&partial_path)) {
                    (Value::Obj(mut base), Some(Value::Obj(extra))) => {
                        base.extend(extra);
                        Value::Obj(base)
                    }
                    (base, _) => base,
                };
                if state.as_obj().is_some() { state } else { object(vec![]) }
            }
        };
        object(vec![("ok", Value::Bool(true)), ("state", resolve_state(state))])
    }
}

/// The tail of the account's log. Spectate shows the game, not the wire:
/// over-fetch the tail (a spammy window is mostly pktlog) then drop the
/// packet lines. The file keeps everything for debug.
fn logs_reply(account: &str, lines: i64) -> Value {
    let (_merged, _partial, log_path) = paths::user_paths(account);
    let lines = lines.max(0) as usize;
    let fetch = lines.saturating_mul(8).max(lines).min(2000);
    let tail = tail_lines(&log_path, fetch);
    let mut game: Vec<String> = tail.into_iter().filter(|line| !is_pktlog_line(line)).collect();
    if game.len() > lines {
        game.drain(..game.len() - lines);
    }
    ok_lines(&game)
}

#[cfg(test)]
#[path = "daemon_commands_tests.rs"]
mod tests;
