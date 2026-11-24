#![forbid(unsafe_code)]
//! What the brain, the market agents and the stats services see of the
//! daemon: the jobs table, the shutdown flag, and the three worker
//! requests they send. `daemon::Orchestrator` implements it; everything
//! above the fleet layer takes `&Arc<dyn Fleet>` instead of naming the
//! daemon (layering step A5, 2026-11-23).

use crate::worker_call::CallError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::Value;

use crate::jobs::Job;

pub trait Fleet: Send + Sync + 'static {
    /// The jobs table (one `Job` per open account).
    fn jobs(&self) -> &Mutex<HashMap<String, Arc<Job>>>;
    /// False once the daemon was told to shut down (service loops stop).
    fn is_running(&self) -> bool;
    /// Does the account's worker process still live?
    fn proc_alive(&self, account: &str) -> bool;
    /// One request to the account's worker socket, one reply.
    fn send_worker(&self, account: &str, request: &Value, timeout: Duration) -> Result<Value, CallError>;
    /// Load and run a script on the account's worker.
    fn run_job(&self, account: &str, script: &str, args: &[Value]) -> Value;
    /// Open the account's session (spawn the worker, wait for its socket).
    fn open_job(self: Arc<Self>, account: &str, no_login: bool) -> Value;
    /// Concrete handle for the daemon's own supervisor thread.
    fn as_any(&self) -> &dyn std::any::Any;
}
