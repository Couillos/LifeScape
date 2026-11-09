#![forbid(unsafe_code)]
//! Single-round-trip plans (E3): the `plan` op replaces the 2 ms poll
//! loop with one server-side wait.
//!
//! Before: `submit` + up to 35 `poll` round-trips per median plan (10
//! tight polls, then one connection per 2 ms), each poll its own Unix
//! connection and daemon thread. Now the client sends one `plan` frame;
//! a bounded `plan-wait` pool thread submits internally, waits on the
//! result in 10 ms slices (no socket traffic), and answers the terminal
//! outcome on the same connection. L1 hits answer on the first slice.
//! Threads stay bounded (16 waiters + 4 compute, queue 256): past that
//! the op refuses loudly and the client falls back to submit/poll.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lifescape_json::{object, str_value, Value};

use crate::plan::{NavOptions, Tile};
use crate::service::{self, PlanClient, Poll};

/// Waiter threads answering `plan` ops (server-side waits, no polls).
/// Scaled from the compute pool (4x): 16 beside the 4 prod lanes, 4 in
/// the single-lane test services (fewer idle threads, same discipline).
pub const WAIT_PER_THREAD: usize = 4;
/// Waiting `plan` streams past this refuse loudly, never queue unbounded.
pub const WAIT_QUEUE: usize = 256;
/// Server-side wait slice: IPC rhythm, not a game wait.
const WAIT_SLICE: Duration = Duration::from_millis(10);
/// Client-side default: covers the 1.5 s exhaustive NoRoutes with margin.
pub const PLAN_ONCE_WAIT_MS: u64 = 8000;
/// Wait slice for the interruptible client loop: the keepalive still
/// ticks between slices and the operator stop aborts within one slice.
/// 500 ms replaces the 2 ms poll: 250x fewer connections, same guards.
pub const PLAN_SLICE_WAIT_MS: u64 = 500;

pub struct WaitJob {
    pub stream: UnixStream,
    pub request: Value,
}

/// Start the bounded waiter pool; the sender lives on `Service`.
/// The pool only calls the `pub(crate)` submit/poll duo, so the
/// acceptor never blocks and threads stay bounded by construction.
pub fn start_wait_pool(service: Arc<service::Service>, threads: usize) -> SyncSender<WaitJob> {
    let (sender, receiver) = mpsc::sync_channel::<WaitJob>(WAIT_QUEUE);
    let shared: Arc<Mutex<Receiver<WaitJob>>> = Arc::new(Mutex::new(receiver));
    for index in 0..threads.saturating_mul(WAIT_PER_THREAD).max(WAIT_PER_THREAD) {
        let worker = service.clone();
        let queue = shared.clone();
        std::thread::Builder::new()
            .name(format!("plan-wait-{index}"))
            .spawn(move || loop {
                let job = {
                    let guard = queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.recv().map_err(|_| ())
                };
                match job {
                    Ok(job) => answer_plan(&worker, job.stream, &job.request),
                    Err(()) => return,
                }
            })
            .expect("plan wait spawns");
    }
    sender
}

/// Server-side `plan` handling, called with the accepted stream:
/// submit internally, wait in slices, answer terminal on the same
/// connection. Runs on a `plan-wait` pool thread, never the acceptor.
/// Protocol: terminal answers release the waiter books (the Ready/Failed
/// slot stays consumable for joins); a slice timeout answers
/// `plan-timeout` WITHOUT cancelling — the client rejoins the same
/// in-flight ticket, never a recompute. Abandoned tickets complete and
/// age out under RESULTS_CAP like any unconsumed terminal.
pub fn answer_plan(service: &service::Service, mut stream: UnixStream, request: &Value) {
    let wait_ms = request.get("wait_ms").as_i64().unwrap_or(PLAN_ONCE_WAIT_MS as i64).max(0) as u64;
    let submit = service.op_submit_public(request);
    let id = match submit {
        Ok(id) => id,
        Err(refused) => {
            let mut buf = lifescape_json::stringify(&refused);
            buf.push('\n');
            let _ = stream.write_all(buf.as_bytes());
            return;
        }
    };
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms.min(15_000));
    loop {
        match service.op_poll_public(id) {
            WaitPoll::Terminal(outcome) => {
                // Release only the waiter count: pool completion already
                // dropped tickets/inflight/admission, and the Ready/Failed
                // slot stays for joins until RESULTS_CAP evicts it.
                service.op_release_waiter(id);
                let mut buf = lifescape_json::stringify(&outcome);
                buf.push('\n');
                let _ = stream.write_all(buf.as_bytes());
                return;
            }
            WaitPoll::Pending => {
                if std::time::Instant::now() >= deadline {
                    // No cancel: the ticket keeps computing (or queueing);
                    // the client rejoins it with the next slice.
                    let reply = object(vec![
                        ("ok", Value::Bool(false)),
                        ("error", str_value("plan-timeout")),
                        ("version", str_value(&service.version_public())),
                    ]);
                    let mut buf = lifescape_json::stringify(&reply);
                    buf.push('\n');
                    let _ = stream.write_all(buf.as_bytes());
                    return;
                }
                std::thread::sleep(WAIT_SLICE);
            }
        }
    }
}

/// One poll from the waiter pool: terminal frames or still pending.
pub enum WaitPoll {
    Terminal(Value),
    Pending,
}

/// Terminal `plan` outcome or a structured refusal. `Refused` carries
/// the served version so the client resubmits once on skew, exactly
/// like submit/poll; `unknown op` signals an old daemon (legacy
/// fallback), every other refusal is `nav-down`.
#[derive(Debug)]
pub enum PlanOnceError {
    Refused { reason: String, served: String },
    Transport(String),
}

impl PlanClient {
    /// One connection, one terminal answer: L1 hits return on the first
    /// server slice, misses wait server-side (no 2 ms polling). `Err`
    /// with `unknown op` means an old daemon: fall back to submit/poll.
    pub fn plan_once(
        &self,
        from: Tile,
        dest: Tile,
        options: &NavOptions,
        version: &str,
        wait_ms: u64,
    ) -> Result<(Poll, String), PlanOnceError> {
        let reply = self
            .round_trip(
                &object(vec![
                    ("op", str_value("plan")),
                    ("account", str_value(self.account_name())),
                    ("from", service::tile_value(from)),
                    ("dest", service::tile_value(dest)),
                    ("options", service::encode_options(options)),
                    ("version", str_value(version)),
                    ("wait_ms", Value::Int(wait_ms as i64)),
                ]),
                Duration::from_millis(wait_ms.saturating_add(5000)),
            )
            .map_err(PlanOnceError::Transport)?;
        if reply.get("ok").as_bool() != Some(true) {
            return Err(PlanOnceError::Refused {
                reason: reply.get("error").as_str().unwrap_or("refused").to_string(),
                served: reply.get("version").as_str().unwrap_or("").to_string(),
            });
        }
        let served = reply.get("version").as_str().unwrap_or("").to_string();
        match reply.get("outcome").as_str().unwrap_or("") {
            "ready" => service::decode_plan(reply.get("plan"))
                .map(|plan| (Poll::Ready(plan), served))
                .map_err(PlanOnceError::Transport),
            "failed" => service::decode_noroute(reply.get("error"))
                .map(|no| (Poll::Failed(no), served))
                .map_err(PlanOnceError::Transport),
            other => Err(PlanOnceError::Transport(format!("bad plan outcome {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PLAN_ONCE_WAIT_MS;

    /// The single wait covers exhaustive NoRoutes with margin.
    #[test]
    fn wait_covers_slow_plans() {
        assert!(PLAN_ONCE_WAIT_MS >= 5000, "median is 70 ms, exhaustive 1.5 s");
    }
}
