#![forbid(unsafe_code)]
//! The commands the daemon sends to a session worker, typed (lot 5.3 of
//! the architecture plan). The JSON stays on the wire (`{"cmd": ...}`,
//! one line, one reply); the daemon builds a `WorkerCommand` and `encode`
//! is the one place that writes its keys, `decode` the one place that
//! reads them back. The worker's own dispatch keeps reading the keys it
//! always read (it changes with the durable channel, lot 5.9).

use lifescape_json::{object, str_value, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum WorkerCommand {
    Ping,
    Status,
    Run { script: String, args: Vec<Value> },
    Unload,
    Pause,
    Resume,
    Hold,
    Close,
    Settle,
    /// `admin`: the account is staff (the worker lists restricted methods).
    Call { method: String, args: Vec<Value>, admin: bool },
    Methods { admin: bool },
    PoseMarket { script: String, args: Vec<String> },
}

impl WorkerCommand {
    /// The `cmd` word.
    pub fn word(&self) -> &'static str {
        match self {
            WorkerCommand::Ping => "ping",
            WorkerCommand::Status => "status",
            WorkerCommand::Run { .. } => "run",
            WorkerCommand::Unload => "unload",
            WorkerCommand::Pause => "pause",
            WorkerCommand::Resume => "resume",
            WorkerCommand::Hold => "hold",
            WorkerCommand::Close => "close",
            WorkerCommand::Settle => "settle",
            WorkerCommand::Call { .. } => "call",
            WorkerCommand::Methods { .. } => "methods",
            WorkerCommand::PoseMarket { .. } => "pose_market",
        }
    }

    pub fn encode(&self) -> Value {
        let mut entries = vec![("cmd", str_value(self.word()))];
        match self {
            WorkerCommand::Run { script, args } => {
                entries.push(("script", str_value(script)));
                entries.push(("args", Value::Arr(args.clone())));
            }
            WorkerCommand::Call { method, args, admin } => {
                entries.push(("method", str_value(method)));
                entries.push(("args", Value::Arr(args.clone())));
                entries.push(("admin", Value::Bool(*admin)));
            }
            WorkerCommand::Methods { admin } => entries.push(("admin", Value::Bool(*admin))),
            WorkerCommand::PoseMarket { script, args } => {
                entries.push(("script", str_value(script)));
                entries.push(("args", Value::Arr(args.iter().map(|arg| str_value(arg)).collect())));
            }
            _ => {}
        }
        object(entries)
    }

    /// Reads a request back (`None`: a word this build does not send).
    pub fn decode(request: &Value) -> Option<WorkerCommand> {
        let args = || request.get("args").as_arr().cloned().unwrap_or_default();
        let admin = request.get("admin").as_bool().unwrap_or(false);
        Some(match request.get("cmd").as_str()? {
            "ping" => WorkerCommand::Ping,
            "status" => WorkerCommand::Status,
            "run" => WorkerCommand::Run { script: request.get("script").as_str().unwrap_or("").to_string(), args: args() },
            "unload" => WorkerCommand::Unload,
            "pause" => WorkerCommand::Pause,
            "resume" => WorkerCommand::Resume,
            "hold" => WorkerCommand::Hold,
            "close" => WorkerCommand::Close,
            "settle" => WorkerCommand::Settle,
            "call" => WorkerCommand::Call { method: request.get("method").as_str().unwrap_or("").to_string(), args: args(), admin },
            "methods" => WorkerCommand::Methods { admin },
            "pose_market" => WorkerCommand::PoseMarket {
                script: request.get("script").as_str().unwrap_or("").to_string(),
                args: args().iter().filter_map(|arg| arg.as_str().map(str::to_string)).collect(),
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
#[path = "worker_command_tests.rs"]
mod tests;
