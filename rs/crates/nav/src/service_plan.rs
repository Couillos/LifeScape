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

use crate::plan::{NavOptions, Tile};
use crate::service::{self, PlanClient, Poll};
use crate::service_ops::{self, PlanRequest, PlanTerminal};

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

/// A `plan` op on its way to the waiter pool: the typed request (or its
/// fault) and the wait the client asked for.
pub struct PlanJob {
    pub request: Result<PlanRequest, String>,
    pub wait_ms: Option<u64>,
}

pub struct WaitJob {
    pub stream: UnixStream,
    pub request: PlanJob,
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
                    Ok(job) => answer_plan(&worker, job.stream, job.request),
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
pub fn answer_plan(service: &service::Service, mut stream: UnixStream, job: PlanJob) {
    let terminal = wait_terminal(service, job.request, job.wait_ms);
    let mut buf = lifescape_json::stringify(&service_ops::encode_terminal(&terminal));
    buf.push('\n');
    let _ = stream.write_all(buf.as_bytes());
}

/// The wait itself, shared by the socket (`answer_plan`) and the
/// in-process caller (`Service::plan_in_process`).
/// Protocol: terminal answers release the waiter books (the Ready/Failed
/// slot stays consumable for joins); a slice timeout answers
/// `plan-timeout` WITHOUT cancelling — the client rejoins the same
/// in-flight ticket, never a recompute. Abandoned tickets complete and
/// age out under RESULTS_CAP like any unconsumed terminal.
pub(crate) fn wait_terminal(service: &service::Service, request: Result<PlanRequest, String>, wait_ms: Option<u64>) -> PlanTerminal {
    let wait_ms = wait_ms.unwrap_or(PLAN_ONCE_WAIT_MS);
    let id = match service.op_submit_public(request) {
        Ok(id) => id,
        Err(refused) => return refused,
    };
    let deadline = std::time::Instant::now() + Duration::from_millis(wait_ms.min(15_000));
    loop {
        let seen = service.completion_mark();
        match service.op_poll_public(id) {
            WaitPoll::Terminal(outcome) => {
                // Release only the waiter count: pool completion already
                // dropped tickets/inflight/admission, and the Ready/Failed
                // slot stays for joins until RESULTS_CAP evicts it.
                service.op_release_waiter(id);
                return outcome;
            }
            WaitPoll::Pending => {
                if std::time::Instant::now() >= deadline {
                    // No cancel: the ticket keeps computing (or queueing);
                    // the client rejoins it with the next slice.
                    return PlanTerminal::Refused { reason: "plan-timeout".to_string(), version: service.version_public() };
                }
                // Woken by the completing search (condvar), the slice
                // only bounds a missed wake-up.
                service.wait_completion(seen, WAIT_SLICE);
            }
        }
    }
}

impl service::Service {
    /// The same `plan` op without the socket (`market_eta` lives in the
    /// daemon, beside the service): short hop first, else the ticket
    /// path and its wait. Same admission, same caches, same counters,
    /// same refusals as a worker's `plan_once`; no JSON frame, no
    /// connection, no `plan-wait` thread.
    pub fn plan_in_process(&self, request: PlanRequest, wait_ms: u64) -> Result<(Poll, String), PlanOnceError> {
        if let Some(terminal) = self.fast_plan(&request) {
            return terminal.into_result();
        }
        wait_terminal(self, Ok(request), Some(wait_ms)).into_result()
    }
}

/// One poll from the waiter pool: terminal frames or still pending.
pub enum WaitPoll {
    Terminal(PlanTerminal),
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
        let request = PlanRequest {
            account: Some(self.account_name().to_string()),
            from,
            dest,
            options: options.clone(),
            version: version.to_string(),
        };
        let reply = self
            .round_trip(&service_ops::encode_plan_op(&request, wait_ms), Duration::from_millis(wait_ms.saturating_add(5000)))
            .map_err(PlanOnceError::Transport)?;
        service_ops::parse_terminal(&reply).map_err(PlanOnceError::Transport)?.into_result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{test_socket, Service};

    /// The single wait covers exhaustive NoRoutes with margin.
    #[test]
    fn wait_covers_slow_plans() {
        assert!(PLAN_ONCE_WAIT_MS >= 5000, "median is 70 ms, exhaustive 1.5 s");
    }

    fn options() -> NavOptions {
        NavOptions {
            allow_paid: false,
            f2p_only: true,
            satisfied: std::collections::HashSet::new(),
            excluded_edges: std::collections::HashSet::new(),
            danger: Vec::new(),
        }
    }

    /// The in-process `plan` answers what the socket `plan` answers, for a
    /// short hop (fast lane), a long plan (ticket path, cache hit on the
    /// second ask), a stale pinned version (typed refusal with the served
    /// version) and a missing account (the ticket path's own fault).
    #[test]
    fn in_process_plan_matches_the_socket_plan() {
        let (snapshot, _world, _tables) = crate::service_replay_tests::real_snapshot();
        let service = Service::start(snapshot, &test_socket("plan-in-process"), 1).expect("service starts");
        let client = PlanClient::new(service.socket_path(), "inproc");
        let version = service.current_version();
        let pairs = [
            (Tile { x: 3228, z: 3214, level: 0 }, Tile { x: 3224, z: 3214, level: 0 }),
            (Tile { x: 3222, z: 3218, level: 0 }, Tile { x: 3165, z: 3487, level: 0 }),
        ];
        for (from, dest) in pairs {
            let over_socket = client.plan_once(from, dest, &options(), &version, 10_000).expect("socket plan");
            let request = PlanRequest { account: Some("inproc".into()), from, dest, options: options(), version: version.clone() };
            let direct = service.plan_in_process(request, 10_000).expect("in-process plan");
            match (over_socket, direct) {
                ((Poll::Ready(left), left_version), (Poll::Ready(right), right_version)) => {
                    assert_eq!(left_version, right_version);
                    assert_eq!(left.cost, right.cost);
                    assert_eq!(service_ops::encode_terminal(&PlanTerminal::Ready { plan: crate::service::encode_plan(&left), version: String::new() }).get("plan"),
                        service_ops::encode_terminal(&PlanTerminal::Ready { plan: crate::service::encode_plan(&right), version: String::new() }).get("plan"));
                }
                ((Poll::Failed(left), _), (Poll::Failed(right), _)) => assert_eq!(left.reason.as_str(), right.reason.as_str()),
                other => panic!("the two paths disagree: {other:?}"),
            }
        }
        let (from, dest) = pairs[1];
        let stale = PlanRequest { account: Some("inproc".into()), from, dest, options: options(), version: "bogus".into() };
        match service.plan_in_process(stale, 1000) {
            Err(PlanOnceError::Refused { reason, served }) => {
                assert!(reason.starts_with("refused:stale-version"), "{reason}");
                assert_eq!(served, version);
            }
            other => panic!("a stale version must refuse, got {other:?}"),
        }
        let nameless = PlanRequest { account: None, from, dest, options: options(), version: String::new() };
        match service.plan_in_process(nameless, 1000) {
            Err(PlanOnceError::Refused { reason, .. }) => assert_eq!(reason, "account"),
            other => panic!("{other:?}"),
        }
    }
}
